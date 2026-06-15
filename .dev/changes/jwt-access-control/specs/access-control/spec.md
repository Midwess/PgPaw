# Delta for Access-Control

Adds JWT authentication and per-request authorization to PgPaw's serving layer. First spec for this domain.

## ADDED Requirements

### Requirement: JWT Authentication

PgPaw SHALL verify an `Authorization: Bearer <jwt>` header on `POST /query` using either HS256 (a configured shared secret) OR RS256/ES256 (a configured PEM public key; JWKS-endpoint auto-resolution by the token's `kid` is planned), extracting the configured role claim and the full claims set. The token's `alg` SHALL match the configured key type (rejecting algorithm-confusion and `none`). A present-but-invalid, expired, or wrong-algorithm token SHALL be rejected with `401`. A request with no token SHALL be treated as anonymous. `GET /q/{hash}/{version}` and `GET /healthz` SHALL remain reachable without a token.

#### Scenario: Valid token yields a principal

- WHEN `POST /query` carries a valid HS256 token with `role` and claims
- THEN the request proceeds with a principal whose role and claims are available to the executor

#### Scenario: Invalid token is rejected

- WHEN `POST /query` carries a token with a bad signature or past its expiry
- THEN PgPaw responds `401` and does not run the query

#### Scenario: Asymmetric token verified via public key / JWKS

- WHEN PgPaw is configured with an RS256/ES256 public key and a request carries a valid asymmetric token
- THEN it is verified using only the public key (no shared secret) and proceeds with a principal

#### Scenario: Algorithm confusion is rejected

- WHEN a token's `alg` does not match the configured key type (e.g. an HS256 token presented against an RS256 key, or `alg: none`)
- THEN PgPaw responds `401`

#### Scenario: Health and snapshot endpoints stay open

- WHEN `GET /healthz` or `GET /q/{hash}/{version}` is requested with no token
- THEN PgPaw serves it normally (no `401`)

### Requirement: Public/Private Classification

PgPaw SHALL classify a read as **public** only if every referenced table has row-level security disabled AND `SELECT` granted to `PUBLIC`; otherwise the read is **access-controlled**. Any ambiguity, error, or unknown table SHALL classify as access-controlled (fail-closed). The per-table verdict SHALL be cached and invalidated whenever `Replica::security_version()` advances.

#### Scenario: Fully public query

- WHEN a query references only RLS-disabled tables that are `SELECT`-granted to `PUBLIC`
- THEN it is classified public

#### Scenario: Any access-controlled table taints the query

- WHEN a query references at least one RLS-enabled or non-`PUBLIC`-granted table
- THEN the whole query is classified access-controlled

#### Scenario: Reclassification on catalog change

- WHEN upstream enables RLS on a previously-public table and the replica's `security_version` advances
- THEN the next classification of a query over that table returns access-controlled, and the stale public verdict is discarded

#### Scenario: Fail-closed on uncertainty

- WHEN classification cannot be determined (unknown table, catalog error)
- THEN the query is treated as access-controlled

### Requirement: Access-Controlled Read Execution

PgPaw SHALL execute an access-controlled read under the principal's role via `query_as` (role + `request.jwt.claims`) and return the result inline on `POST /query`. An access-controlled read SHALL NEVER be served via the `303 → /q/{hash}/{version}` snapshot path and SHALL NEVER be stored in the query cache. An access-controlled read with no/invalid token SHALL be `401`; one whose role is denied (or does not exist) SHALL be `403`.

#### Scenario: Scoped rows under the role

- WHEN an authenticated principal runs an access-controlled query
- THEN only the rows the role's RLS policies permit are returned, inline with `Cache-Control: private, no-store`

#### Scenario: Never reaches the public snapshot

- WHEN an access-controlled query is served
- THEN no `303` redirect is issued and nothing is written to the query cache

#### Scenario: Anonymous access-controlled request is rejected

- WHEN an access-controlled query is requested with no token
- THEN PgPaw responds `401`

#### Scenario: Denied role maps to 403

- WHEN `query_as` fails because the role lacks privilege or does not exist (sqlstate `42501` / `42704` / `28000`)
- THEN PgPaw responds `403` (never a superuser fallback, never `500`)

### Requirement: Cacheability Policy

A public read SHALL be served via the existing snapshot path with `Cache-Control: public, max-age=259200` (72 hours) and an `ETag`. An access-controlled read SHALL be served with `Cache-Control: private, no-store`. Public snapshots SHALL use a bounded `max-age` (not `immutable`) so that a table reclassified public→private stops being served from shared caches within the TTL.

#### Scenario: Public snapshot is CDN-cacheable but bounded

- WHEN a public query's snapshot is served at `GET /q/{hash}/{version}`
- THEN the response carries `Cache-Control: public, max-age=259200` and an `ETag`

#### Scenario: Live private is rejected

- WHEN `POST /query?live=true` is access-controlled
- THEN PgPaw responds `403` (private live deltas are out of scope)
