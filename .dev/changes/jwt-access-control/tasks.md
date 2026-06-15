# Tasks: jwt-access-control

## Progress: [0/25]

## 1. Errors + config

- [ ] 1.1 `error.rs`: add `CacheError::Unauthorized(String)` (→401) and `Forbidden(String)` (→403); extend `name()`.
- [ ] 1.2 `http/query.rs` `error_response`: map `Unauthorized → 401`, `Forbidden → 403` (no `ResponseError` impl).
- [ ] 1.3 `Cargo.toml`: add `jsonwebtoken = "9"` (HS256 + RS256/ES256 decode) + a JWKS fetch/cache (lightweight HTTP client + in-memory key-by-`kid` cache).
- [ ] 1.4 `main.rs` `Options`: add one-of verification key — `--jwt-secret`/`JWT_SECRET` (HS256), `--jwt-public-key`/`JWT_PUBLIC_KEY` (PEM, RS256/ES256), `--jwt-jwks-url`/`JWT_JWKS_URL` — plus `--jwt-role-claim`/`JWT_ROLE_CLAIM` (default `role`); thread into `ServerConfig` (`di.rs`). Validate exactly one key source is set.

## 2. Auth module (`src/auth.rs`, new)

- [ ] 2.1 `Principal { role, claims_json }`.
- [ ] 2.2 Verify helper: pin the allowed algorithm to the configured key type — HS256 (secret) or RS256/ES256 (PEM public key); validate `exp`; extract the configured role claim; re-serialize full claims to JSON for `request.jwt.claims`. Reject any token whose `alg` doesn't match the configured key (no alg-confusion).
- [ ] 2.3 `OptionalPrincipal` `FromRequest`: no header → `Ok(None)`; valid → `Ok(Some)`; present-but-invalid/expired/wrong-alg → `Err(Unauthorized)`. `mod auth` in `lib.rs`.
- [ ] 2.4 Unit tests (pure, known keys): HS256 valid → role+claims; RS256 valid (test keypair) → role+claims; bad signature → 401; expired → 401; `alg` mismatch (HS256 token against an RS key, or `none`) → 401; missing role claim → 401/handled.
- [ ] 2.5 JWKS mode: fetch the JWKS document from `--jwt-jwks-url`, select the key by the token's `kid`, cache in-memory, refresh on unknown `kid` (bounded). Verify RS256/ES256 against the resolved key.

## 3. Classifier verdict (`di.rs`)

- [ ] 3.1 `Di` + `ServerConfig`: `jwt_secret`, `jwt_role_claim`, `security_cache: Arc<Mutex<(u64, HashMap<String,bool>)>>`; accessors.
- [ ] 3.2 `Di::is_private(&self, tables) -> Result<bool, CacheError>`: read `replica.security_version()`; lock `(version,map)` as a unit; on version change clear + re-query local catalog (`relrowsecurity` + `has_table_privilege('public', oid, 'SELECT')`) per table; **fail-closed (private) on any error/unknown**; cache per-table.
- [ ] 3.3 Unit-testable seam: the verdict-merge logic (`any table private ⇒ private`) as a pure helper where feasible.

## 4. Execution (`rows.rs`)

- [ ] 4.1 `query_json_as(db, role, claims, sql)` — `db.query_as(role, Some(claims), &wrap_json(sql), &[])`, same row→json extraction as `query_json`.

## 5. Routing (`http/query.rs`)

- [ ] 5.1 `query(params, body, principal: OptionalPrincipal)`; pass principal to `materialize`.
- [ ] 5.2 `materialize`: after classify, `di.is_private(&tables).await?` branch.
- [ ] 5.3 Public branch: existing version→cache→`303` path unchanged; change `cursor` snapshot header to `Cache-Control: public, max-age=259200`.
- [ ] 5.4 Private branch: require `Some(principal)` else `Unauthorized`; `query_json_as`; return **inline `200`** + `Cache-Control: private, no-store`; never cache, never `303`. Map `pglite::Error::Database{sqlstate}` `42501`/`42704`/`28000` → `Forbidden`.
- [ ] 5.5 `?live=true` + private → `Forbidden` (live is v2 public-only).

## 6. Testing

- [ ] 6.1 No/invalid token + private query → 401/403; missing token + public query → 200 (anonymous public allowed).
- [ ] 6.2 Public query → `303` → snapshot `public, max-age=259200` (CDN path intact).
- [ ] 6.3 Private query + valid token → rows scoped by RLS (via `query_as`); **inline 200, `private, no-store`, no `303`, not in `QueryCache`** (anti-leak invariant).
- [ ] 6.4 Fail-closed: an unclassifiable / RLS / non-PUBLIC table → treated private.
- [ ] 6.5 Denied role (sqlstate `42501`/`42704`) → 403 (not 500, not superuser fallback).
- [ ] 6.6 `/healthz` and `GET /q/{hash}/{version}` remain reachable without a token.
- [ ] 6.7 `?live=true` + private → 403.

---

## Notes
- Depends on the `pglite-rs` path dep (`query_as` + `security_version`). Revert to a published version before release.
- Behavioral tests that exercise `query_as`/RLS need the replica streaming from a live upstream (same gate as `replica-access-control`). Pure auth/verdict logic is unit-testable without a DB.
- `live.rs` unchanged (public-only); private live deltas are v2.
