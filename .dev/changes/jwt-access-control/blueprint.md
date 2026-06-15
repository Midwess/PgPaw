# Architecture Blueprint: jwt-access-control

Synthesized from the code-explorer map + the locked design. Extends the existing request pipeline; one new module (`auth`), the rest attaches to existing structs.

## Request flow (after)

```
POST /query  (FromRequest: Option<Principal> from HS256 bearer)
   │
   ▼ materialize(sql, principal)
   classify(sql) ─► CacheableQuery { tables, ... }
   │
   ▼ di.is_private(&tables).await           (verdict cached, keyed by Replica::security_version())
   ├── public ──► version_of → key → QueryCache.get_or_compute(query_json)
   │              └─► 303 → /q/{hash}/{version}     [GET /q: public, max-age=259200, ETag — unauthenticated, CDN]
   └── private ─► require Some(principal) else 401
                  rows::query_json_as(db, role, claims, sql)   (SET LOCAL ROLE + request.jwt.claims, via query_as)
                  └─► 200 inline JSON, Cache-Control: private, no-store   (never cached, never redirected)
                      pglite Error sqlstate 42501/42704/28000 → 403
```

## New / changed interfaces

### `src/auth.rs` (new — the only new module; justified new domain)
```rust
pub struct Principal {
    pub role: String,
    pub claims_json: String,   // full claims, serialized for request.jwt.claims
}

// FromRequest: reads Authorization: Bearer; verifies HS256 against Di::instance().jwt_secret();
// extracts the configured role claim + re-serializes claims. Returns:
//   Ok(None)        -> no Authorization header (anonymous; allowed for public)
//   Ok(Some(p))     -> valid token
//   Err(401)        -> present but invalid/expired/malformed
impl FromRequest for OptionalPrincipal { /* Future<Output=Result<Option<Principal>, CacheError>> */ }
```
No verification when `jwt_secret` is unconfigured ⇒ all requests anonymous (public-only) — a valid dev/deploy posture.

### `src/di.rs`
- `ServerConfig` += `jwt_secret: Option<String>`, `jwt_role_claim: String` (default `"role"`).
- `Di` += `jwt_secret: Option<String>`, `jwt_role_claim: String`, `security_cache: Arc<Mutex<(u64, HashMap<String, bool>)>>`.
- Accessors `jwt_secret() -> Option<&str>`, `jwt_role_claim() -> &str`.
- New method (the verdict + invalidation):
```rust
pub async fn is_private(&self, tables: &[String]) -> Result<bool, CacheError> {
    let v = self.replica.security_version().await?;
    // lock (version, map) as a unit; if v != cached version, clear + re-check tables
    // for each table not in map: query local catalog:
    //   SELECT bool_or(c.relrowsecurity) OR bool_or(NOT has_table_privilege('public', c.oid, 'SELECT'))
    //   over the referenced tables  ->  any true ⇒ private
    // cache per-table; fail-closed (true) on any error/unknown.
}
```

### `src/rows.rs`
```rust
pub async fn query_json_as(db: &PGlite, role: &str, claims: &str, sql: &str) -> Result<String, CacheError> {
    let rows = db.query_as(role, Some(claims), &wrap_json(sql), &[]).await?;
    // same row→json extraction as query_json
}
```

### `src/http/query.rs`
- `query(params, body, principal: OptionalPrincipal)` — pass `principal` into `materialize`.
- `materialize`: after `classify`, branch on `di.is_private(&query.tables).await?`:
  - public → existing path (unchanged), but the **303 path is unchanged** and the **`cursor` snapshot header changes to `public, max-age=259200`**.
  - private → `principal` required (else `CacheError::Unauthorized`); `query_json_as`; return inline `200` + `private, no-store`; map sqlstate → `Forbidden`.
  - private + `params.live` → `Forbidden` (v2).
- `error_response` += `Unauthorized → 401`, `Forbidden → 403`.

### `src/error.rs`
- `CacheError::Unauthorized(String)`, `CacheError::Forbidden(String)`; `name()` arms; status mapping in `error_response` (no `ResponseError` impl — matches existing).

### `src/main.rs`
- `Options` += `#[arg(long="jwt-secret", env="JWT_SECRET", global=true)] jwt_secret: Option<String>` and `#[arg(long="jwt-role-claim", env="JWT_ROLE_CLAIM", global=true, default_value="role")] jwt_role_claim: String`; thread into `ServerConfig`.

## Files
**Create:** `src/auth.rs`.
**Modify:** `src/di.rs`, `src/http/query.rs`, `src/rows.rs`, `src/error.rs`, `src/main.rs`, `src/lib.rs` (declare `mod auth`), `Cargo.toml`.
**Review (no change):** `src/http/server.rs` (extractor is per-handler), `src/cache.rs` (public path only), `src/version.rs`, `src/live.rs` (stays public-only).

## Phases
1. **Errors + config** — `Unauthorized`/`Forbidden` + `error_response`; `jwt_secret`/`jwt_role_claim` through `Options`→`ServerConfig`→`Di`. (No behavior yet.)
2. **Auth module** — `Principal` + HS256 verify + `OptionalPrincipal` extractor. Unit-test verify/expiry/role-extraction (pure, with a known secret).
3. **Classifier verdict** — `Di::is_private` + `security_cache` (version-keyed, fail-closed). 
4. **Execution** — `rows::query_json_as`.
5. **Routing** — wire `materialize` public/private branch + inline private response + 72h public TTL + `?live=true`+private→403.
6. **Tests** — auth (401), public anon (cacheable), private enforcement via `query_as`, fail-closed classification, sqlstate→403, `/q/` stays open, live+private→403.

## Risks → mitigations
| Risk | Mitigation |
|------|------------|
| private snapshot reaches the public `/q/` cache | private path returns inline only; never `get_or_compute`, never 303 |
| misclassification | fail-closed (private) on any ambiguity/error |
| stale public edge after reclassify | `security_version` evicts origin + reclassifies; 72h caps edge residual |
| verdict cache race | lock `(version,map)` unit; re-check version inside lock |
| wrong status on denied role | sqlstate `42501`/`42704`/`28000` → 403 |
