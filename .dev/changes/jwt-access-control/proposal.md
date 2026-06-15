# Proposal: JWT Authentication + Per-Request Authorization

**Status**: approved

## Summary

Add HS256 JWT authentication and per-request authorization to PgPaw. PgPaw verifies a bearer token, classifies each read as **public** or **access-controlled** against the replica's now-replicated security catalog, and executes access-controlled reads under the JWT's role via `PGlite::query_as` (so RLS/grants apply). Public reads keep the global CDN-cacheable snapshot path; access-controlled reads run live and uncached. This is the serving-side consumer of the `replica-access-control` change.

## Motivation

PgPaw currently executes every read as the `postgres` superuser with no principal and no auth — anyone who can reach the port reads everything, RLS-blind. The replica now holds RLP (roles/grants/RLS/policies) and exposes `query_as(role, claims, …)` + `security_version()`. PgPaw must (1) authenticate callers, (2) decide which reads are safe to serve globally vs must be scoped to the caller, and (3) execute scoped reads under the caller's role so the database enforces access control.

## Scope

### In Scope
- **Auth extractor (`src/auth.rs`, new):** `FromRequest` on `POST /query` only. **Bring-your-own-JWT, verify-only**: verify `Authorization: Bearer <jwt>` with either an **HS256 shared secret** OR an **RS256/ES256 public key / JWKS endpoint** (the customer supplies whichever their existing auth uses). Produce `Option<Principal { role, claims_json }>` (anonymous allowed). Invalid/expired token → 401.
- **Public/private classifier:** `Di::is_private(&tables)` consults the replicated catalog — a query is **public** iff every referenced table has RLS off AND `SELECT` granted to `PUBLIC`; else private; unprovable/error → private (fail-closed). Table-level (v1).
- **Verdict cache:** per-table `(schema.table → is_private)` map on `Di`, snapshotting `Replica::security_version()`; refreshed when it advances (the cache-invalidation consumer of `security_version`).
- **Routing (`materialize`):** public → existing classify→version→`QueryCache`→`303 /q/{hash}/{version}` path; access-controlled → `rows::query_json_as(db, role, claims, sql)` returned **inline** (`200`), never redirected, never cached.
- **Access rule:** access-controlled query with no/invalid token → 401; with a token whose role can't read → 403. Anonymous is allowed for public queries.
- **Cacheability:** public response `Cache-Control: public, max-age=259200` (72h) + `ETag`; access-controlled response `Cache-Control: private, no-store`.
- **Errors:** `CacheError::Unauthorized`→401, `CacheError::Forbidden`→403; mapped in `error_response`.
- **Config:** exactly one verification key — `--jwt-secret`/`JWT_SECRET` (HS256) **or** `--jwt-public-key`/`JWT_PUBLIC_KEY` (PEM, RS256/ES256) **or** `--jwt-jwks-url`/`JWT_JWKS_URL` (fetch+cache JWKS by `kid`); plus `--jwt-role-claim`/`JWT_ROLE_CLAIM` (default `role`). Dep: `jsonwebtoken = "9"` (HS/RS/ES decode) + a small JWKS fetch/cache.
- **Claims contract (doc):** the customer's JWT claim keys must match what their RLS policies read (`current_setting('request.jwt.claims',true)::json->>'<key>'`), and the `role` claim must name a replicated non-superuser DB role — claim keys need NOT match column names (the policy is the bridge). Recommend **self-contained policies** (inline `current_setting`, no helper-function dependency); replicating helper functions like `auth.uid()` is v2. See `research-rls.md`.

### Out of Scope (v2)
- Private **SSE live deltas** (`live.rs` stays public-only; `?live=true` + private → 403).
- Replicating policy **helper functions** (`auth.uid()` etc.) onto the replica — v1 requires self-contained policies (inline `current_setting`).
- Column-level grant classification (table-level only, mirroring the replica side).
- CDN purge-on-`security_version` (the 72h TTL bounds residual staleness instead).

## Affected Areas

| Area | Impact |
|------|--------|
| `src/auth.rs` (new) | `Principal` + HS256 verify + `FromRequest` extractor |
| `src/http/server.rs` | (extractor is per-handler; no `wrap()` — `/healthz` + `/q/` stay open) |
| `src/http/query.rs` | `query`/`materialize` gain the public/private branch + the private inline path; public TTL → 72h; `error_response` gains 401/403 |
| `src/di.rs` | `jwt` config + `security_cache` + `is_private()` + accessors; `ServerConfig` gains `jwt_secret`/`jwt_role_claim` |
| `src/rows.rs` | new `query_json_as(db, role, claims, sql)` |
| `src/error.rs` | `Unauthorized`/`Forbidden` variants + `name()` |
| `src/main.rs` | `Options` gains `--jwt-secret` / `--jwt-role-claim` |
| `Cargo.toml` | `jsonwebtoken = "9"` |

## Dependencies
- `pglite-rs` path dep providing `query_as` + `security_version` (in place). **Revert to a published version before release.**
- `replica-access-control` (the pglite-rs change) must be live for `query_as`/catalog to mean anything — currently uncommitted + unverified against a real upstream.
- New crate `jsonwebtoken = "9"`.

## Risks

| Risk | Mitigation |
|------|------------|
| `/q/` cursor is public + unauthenticated; a private snapshot there would leak | Private queries never 303 / never enter `QueryCache`; inline JSON only. |
| Public/private misclassification → unauth'd access | Fail-closed: ambiguous/error → private. |
| CDN serves a stale public snapshot after a table flips public→private (can't purge edge) | `security_version` bump re-classifies + evicts origin cache immediately; 72h `max-age` bounds the edge residual. |
| `security_cache` read-then-refresh race | Lock `(version, map)` as a unit; re-check version inside the lock. |
| Wrong status for `query_as` errors | sqlstate `42501`/`42704`/`28000` → 403; else 500. |
| Auth on `/healthz` breaks LB probes | Extractor on `query` only. |
| `?live=true` + private leaks rows | classify first; private + live → 403. |
