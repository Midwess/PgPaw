# Design: jwt-access-control

## Overview
Authenticate callers (HS256 JWT), classify each read public vs access-controlled against the replica's replicated security catalog, and execute access-controlled reads under the caller's role via `query_as`. Public reads keep the global CDN-cacheable snapshot; access-controlled reads run live, uncached.

## Key Decisions

### Decision 1 — Cacheability follows the classifier verdict, not the token
**Context.** Earlier shorthand was "Authorization present → private." That over-privatizes public data and under-protects private data sent without a token.
**Decision.** The **classifier verdict alone** decides the path + `Cache-Control`; the **token only gates access** to private data.
- Public query → `public, max-age=259200` snapshot (CDN), regardless of token (public data is public).
- Access-controlled query → `query_as` under the role, `private, no-store`.
- Access-controlled query + no/invalid token → 401/403.
This keeps the cache key a pure function of the query+version (CDN-safe) and never depends on hidden session state.

### Decision 2 — Private reads return inline; they never touch the `/q/` snapshot path
**Context.** `GET /q/{hash}/{version}` is public, CDN-cached, and unauthenticated by design (it's the immutable-snapshot value prop).
**Decision.** A private query is executed via `query_as` and its JSON is returned **inline on `POST /query` (`200`)**, never via a `303` redirect and never inserted into `QueryCache`. So a private result can never end up at a public, unauthenticated, CDN-cached URL. This is the load-bearing anti-leak invariant.

### Decision 3 — `FromRequest` extractor on `/query`, not blanket middleware
**Context.** `/healthz` (LB probes) and `/q/` (public snapshots) must stay unauthenticated; PgPaw has no middleware today and a global `Di` singleton (no `web::Data`).
**Decision.** Verify the token in a `FromRequest` extractor used only by the `query` handler, yielding `Option<Principal>` (anonymous allowed). Keeps 401 production local, leaves the other routes open, and matches the existing no-middleware style.

### Decision 4 — Public ⟺ "no RLS AND SELECT-to-PUBLIC on every referenced table"; fail-closed
**Context.** A result is shareable only if it's identical for everyone.
**Decision.** `Di::is_private(&tables)` returns false (public) only when every referenced table has `relrowsecurity = false` AND `has_table_privilege('public', oid, 'SELECT')`. Anything else, or any ambiguity/error/unknown table, → private. RLS-enabled ⇒ private (we never evaluate policy expressions). Table-level for v1 (column grants deferred on the replica side too).

### Decision 5 — `security_version` is the verdict-cache invalidator + the 72h TTL bounds CDN residual
**Context.** Re-querying the catalog per request is wasteful; and a table can flip public→private after snapshots are already at the CDN edge (which the origin can't purge).
**Decision.** Cache the per-table verdict keyed on `Replica::security_version()`; a bump (any upstream policy/grant/role change) clears it and re-classifies, and PgPaw evicts its own origin cache. Edge copies marked `public` persist up to their `max-age` — so public snapshots use **`max-age=259200` (72h)**, not `immutable, 1y`, to bound that residual exposure window. Data freshness itself is still handled by versioned URLs, not the TTL.

### Decision 6 — Bring-your-own-JWT: HS256 secret OR RS256/ES256 public key/JWKS (verify-only)
**Context.** Customers already have an auth system; PgPaw should accept their existing tokens, not impose a new issuer. Self-issued auth is usually HS256 (a shared secret); third-party IdPs (Auth0/Cognito/Clerk/Keycloak) are RS256/ES256 with a JWKS endpoint and no shareable symmetric secret.
**Decision.** Support **both** verification modes, configured one-of: HS256 shared secret, RS256/ES256 PEM public key, or a JWKS URL (fetch + cache by `kid`, refresh on unknown `kid`). PgPaw only **verifies** — it never mints. Asymmetric is also the safer default (PgPaw holds only a public key, never the minting authority). The decoded claims flow unchanged into `SET LOCAL ROLE` + `request.jwt.claims`; the verification mode does not affect anything downstream.

### Claims contract
The customer owns the contract between their **token issuer** and their **RLS policies**; PgPaw is the courier.
- **`role` claim** → must name a replicated, non-superuser DB role (selects which `TO <role>` policies fire). The only claim coupled to the DB catalog.
- **Other claim keys** → must match the `->>'<key>'` literals their policies read; they need **not** match table column names (the policy maps claim→column). PgPaw forwards the whole verified blob into `request.jwt.claims` and interprets nothing but `role`.
- **Self-contained policies** (recommended v1): read claims inline via `current_setting('request.jwt.claims', true)::json->>'…'`, no helper-function dependency — so `reconcile_security` (which replicates policies but not functions) reconstructs them cleanly. Helper-function replication (`auth.uid()`-style) is v2.
- Policy-authoring best practices (initPlan `(select …)` wrap, index policy columns, RESTRICTIVE tenant policy + permissive per-role SELECT, default-deny): see `research-rls.md`.

## Data Model
- `Principal { role: String, claims_json: String }` (crosses extractor → handler → `query_as`; justified struct).
- `Di.security_cache: Arc<Mutex<(u64, HashMap<String, bool>)>>` — `(security_version_seen, table → is_private)`.
- `ServerConfig.jwt_secret: Option<String>`, `ServerConfig.jwt_role_claim: String`.

## API / Config Changes
- `POST /query` now reads `Authorization: Bearer <jwt>` (HS256). Public response: `public, max-age=259200` + `ETag`. Private response: `200` inline + `private, no-store`. New statuses: `401`, `403`.
- `GET /q/{hash}/{version}` unchanged except `max-age` 1y→72h; remains unauthenticated (only ever holds public data).
- New flags: `--jwt-secret`/`JWT_SECRET`, `--jwt-role-claim`/`JWT_ROLE_CLAIM` (default `role`). Unconfigured secret ⇒ anonymous-only (public-only) operation.

## Security Considerations
- **Anti-leak invariant** (Decision 2): private never reaches `/q/`/`QueryCache`. A test asserts `query_json_as` never writes the cache and private requests return `200` inline, not `303`.
- **Fail-closed** (Decision 4): misclassification defaults to private (denies/scopes), never to public.
- **No-secret posture:** with `jwt_secret` unset, every request is anonymous → only public queries succeed; private → 401. Safe default.
- **Token role → DB role:** `query_as` runs as a non-superuser role; `SET LOCAL` resets at txn end (pglite side). Unknown/denied role → sqlstate `42501`/`42704`/`28000` → 403, never a superuser fallback.
- **Residual CDN window:** bounded to 72h (Decision 5); optionally tightened later with a CDN purge hook.
- **Trust note:** this still depends on the replica actually enforcing RLS via `query_as` — which is uncommitted + unverified against a live upstream as of this proposal.
