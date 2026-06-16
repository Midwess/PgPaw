---
id: jwt-verification-modes
title: JWT verifier accepts only correctly-signed well-formed HS256 tokens and rejects every malformed/forged/mis-configured variant with 401, while a no-jwt server refuses presented tokens yet still serves public queries
tier: live
component: auth
target: pgpaw (cache-server binary)
prerequisites:
  - "FRESH WORKSPACE REQUIRED — every live test MUST run against freshly-created workspaces (do NOT reuse a previously-tested data-dir or a leftover docker container). A stale --data-dir replica or container contaminates the auth observations."
  - "`docker --version` returns a version string"
  - "`curl --version` returns a version string"
  - "`cargo --version` returns a version string (PgPaw built from this repo, or `cache-server` on PATH)"
  - "No container named `pgpaw-jwt-pg` already exists (`docker ps -a --filter name=pgpaw-jwt-pg --format '{{.Names}}'` prints nothing)"
  - "TCP port 55435 on 127.0.0.1 is free (`lsof -i :55435` prints nothing)"
  - "TCP port 8083 on 127.0.0.1 is free (`lsof -i :8083` prints nothing)"
  - "TCP port 8084 on 127.0.0.1 is free (`lsof -i :8084` prints nothing)"
  - "Data dir `/tmp/pgpaw-jwt-data` does not yet exist (`test -d /tmp/pgpaw-jwt-data` returns non-zero)"
  - "Data dir `/tmp/pgpaw-jwt-data-noauth` does not yet exist (`test -d /tmp/pgpaw-jwt-data-noauth` returns non-zero)"
expected_duration_secs: 480
tags: [jwt, auth, verification, alg-none, algorithm-downgrade, access-control, live]
priority: high
created: 2026-06-16
author: senior-qa
---

## Objective

Verify that PgPaw's JWT verifier accepts only a correctly-signed, unexpired HS256 token carrying a string `role` claim and rejects every malformed/forged/mis-configured variant (expired, bad-signature, `alg:none` forgery, missing-role, no-`Bearer` prefix, wrong scheme) with `401` and no leaked rows, that public queries stay token-free on any server, that a server launched with NO jwt config refuses presented tokens (`401` not-configured) while still serving public queries, and that `--jwt-jwks-url` fails loud at startup.

## Preconditions

- `docker ps` succeeds (daemon reachable)
- The fixed HS256 secret below (`pgpaw-test-secret-please-change`) will be passed to the PRIMARY PgPaw via `--jwt-secret`; the tokens in Inputs are pre-signed against it
- No container named `pgpaw-jwt-pg` already exists (`docker ps -a --filter name=pgpaw-jwt-pg --format '{{.Names}}'` prints nothing)
- Data dir `/tmp/pgpaw-jwt-data` does not yet exist (`test -d /tmp/pgpaw-jwt-data` returns non-zero)
- Data dir `/tmp/pgpaw-jwt-data-noauth` does not yet exist (`test -d /tmp/pgpaw-jwt-data-noauth` returns non-zero)

## Inputs

Fixed HS256 secret the PRIMARY PgPaw is launched with:

```text
pgpaw-test-secret-please-change
```

Pre-signed test tokens (reused verbatim from the `jwt-rls-enforcement` exemplar, signed against the secret above; `role=member`, `org_id=1`):

```text
# Valid HS256, role=member, org_id=1, long-lived exp — the ONE shape that must be accepted
TOKEN_A=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoxLCJleHAiOjIwOTY5MDQ2MzF9.KGb25waEy8TTsaWqOzsENgQ8wU0EkyMjrCFiX3NHhDI

# Expired (exp in the past), otherwise well-formed and validly signed — 401
TOKEN_EXPIRED=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoxLCJleHAiOjE3ODE1NDEwMzF9.slXfdxqROE0gZm_IL73fHWiavjCT1Mf1daCL6XyIW24

# Bad signature (A's claims signed with the wrong secret) — 401
TOKEN_BADSIG=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoxLCJleHAiOjIwOTY5MDQ2MzF9.3Q9qFFqMZBsC4sNP4Nj5SM_-eLuEJ-iO0LOqSDVWniU
```

Two additional pre-minted tokens for the structural/algorithm edges (also against `pgpaw-test-secret-please-change`, long exp):

```text
# Valid HS256 signature but NO 'role' claim — 401 (token lacks role)
MISSING_ROLE=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJvcmdfaWQiOjEsImV4cCI6MjA5NjkwNDYzMX0.62S2Y-E6-PQHKUJoitNKI1QSda5fMJUdiFxo594a-b0

# alg:none forgery (header alg=none, empty signature) — classic algorithm-downgrade attack; MUST be 401
ALG_NONE=eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoxLCJleHAiOjIwOTY5MDQ2MzF9.
```

Schema + seed — applied one statement per Step in `## Steps`:

```sql
-- public table: RLS off, granted to PUBLIC (no token needed)
CREATE TABLE pub_t (id int PRIMARY KEY, v text);
INSERT INTO pub_t VALUES (1,'pub-one'), (2,'pub-two');
GRANT SELECT ON pub_t TO PUBLIC;

-- private table: RLS on, scoped by org_id claim
CREATE TABLE secret_t (id int PRIMARY KEY, org_id int, v text);
INSERT INTO secret_t VALUES (11,1,'org1-a'), (12,1,'org1-b');
CREATE ROLE member LOGIN;
GRANT SELECT ON secret_t TO member;
ALTER TABLE secret_t ENABLE ROW LEVEL SECURITY;
ALTER TABLE secret_t FORCE ROW LEVEL SECURITY;
CREATE POLICY secret_by_org ON secret_t FOR SELECT TO member
  USING ( org_id = ((select current_setting('request.jwt.claims', true))::json->>'org_id')::int );

-- publication PgPaw replicates
CREATE PUBLICATION cache_server_pub FOR ALL TABLES;
```

Ground truth the tester judges row results against:

```text
pub_t  (public): ids 1, 2
secret_t org_id=1 owns ids: 11, 12   (the only rows a member/org_id=1 token may ever see)
```

## Steps

> **Notebook discipline**: Each Step is ONE action. The tester runs it, observes, judges, then moves to the next. No `&&`, `;`, multi-line shells, or for-loops inside a Step's Action. Polling = a Step the tester runs N times.

### Step 1: Provision a fresh logical-replication Postgres

**Action**: `docker run -d --name pgpaw-jwt-pg -e POSTGRES_PASSWORD=postgres -p 55435:5432 postgres:16 -c wal_level=logical`

**Observe**:
- Command exits 0 and prints a 64-hex container id
- `docker ps --filter name=pgpaw-jwt-pg` shows the container as Up

**Awareness**:
- This pulls `postgres:16` on first run — a slow pull is normal, not a failure. A non-zero exit citing "port is already allocated" means port 55435 was not actually free; abort and re-check the precondition.
- Confirm no OTHER postgres container is bound to 55435 that could shadow this one.

**On weirdness**: abort

### Step 2: Confirm wal_level is logical

**Action**: `docker exec pgpaw-jwt-pg psql -U postgres -tAc "SHOW wal_level"`

**Observe**:
- Output is exactly `logical`

**Awareness**:
- If this prints `replica` the `-c wal_level=logical` flag did not take — replication never starts and every private query looks empty (a 503 from a halted replica, NOT the 401 this test cares about). Treat a wrong value as a hard stop.
- A "FATAL: the database system is starting up" error means Postgres is not ready yet; wait a few seconds and retry.

**On weirdness**: retry once (for startup race); abort if value is not `logical`

### Step 3: Create the public table

**Action**: `docker exec pgpaw-jwt-pg psql -U postgres -c "CREATE TABLE pub_t (id int PRIMARY KEY, v text)"`

**Observe**:
- Output is `CREATE TABLE`

**Awareness**:
- Watch for any NOTICE about an existing relation — that would mean the container is not actually fresh.

**On weirdness**: abort

### Step 4: Seed the public rows

**Action**: `docker exec pgpaw-jwt-pg psql -U postgres -c "INSERT INTO pub_t VALUES (1,'pub-one'),(2,'pub-two')"`

**Observe**:
- Output is `INSERT 0 2`

**Awareness**:
- A row count other than 2 means a partial seed; the public-query assertions become unreliable.

**On weirdness**: abort

### Step 5: Grant public read on pub_t

**Action**: `docker exec pgpaw-jwt-pg psql -U postgres -c "GRANT SELECT ON pub_t TO PUBLIC"`

**Observe**:
- Output is `GRANT`

**Awareness**:
- This is what makes a `pub_t` query classify PUBLIC (no token needed). If it is missed, the no-token public queries (Steps 16 and 21) would wrongly demand a token.

**On weirdness**: abort

### Step 6: Create the private table

**Action**: `docker exec pgpaw-jwt-pg psql -U postgres -c "CREATE TABLE secret_t (id int PRIMARY KEY, org_id int, v text)"`

**Observe**:
- Output is `CREATE TABLE`

**Awareness**:
- A missing-relation or duplicate error here means the container is not fresh; abort.

**On weirdness**: abort

### Step 7: Seed the private rows (org 1)

**Action**: `docker exec pgpaw-jwt-pg psql -U postgres -c "INSERT INTO secret_t VALUES (11,1,'org1-a'),(12,1,'org1-b')"`

**Observe**:
- Output is `INSERT 0 2`

**Awareness**:
- These are the only rows a valid org_id=1 token may ever return; if the count is not 2, the "valid token accepted" sanity (Step 17) is meaningless.

**On weirdness**: abort

### Step 8: Create the non-superuser login role

**Action**: `docker exec pgpaw-jwt-pg psql -U postgres -c "CREATE ROLE member LOGIN"`

**Observe**:
- Output is `CREATE ROLE`

**Awareness**:
- `member` must NOT be superuser and must NOT have BYPASSRLS — either makes RLS a no-op. A "role already exists" notice means a non-fresh container.

**On weirdness**: abort

### Step 9: Grant member read on secret_t

**Action**: `docker exec pgpaw-jwt-pg psql -U postgres -c "GRANT SELECT ON secret_t TO member"`

**Observe**:
- Output is `GRANT`

**Awareness**:
- Without this grant, even the valid token in Step 17 hits a privilege error and surfaces as a 403, not the expected 200 with org-1 rows.

**On weirdness**: abort

### Step 10: Enable RLS on secret_t

**Action**: `docker exec pgpaw-jwt-pg psql -U postgres -c "ALTER TABLE secret_t ENABLE ROW LEVEL SECURITY"`

**Observe**:
- Output is `ALTER TABLE`

**Awareness**:
- Enabling RLS is exactly what flips `secret_t` from public to access-controlled in PgPaw's classifier. After this, a query touching `secret_t` is private and demands a verified token.

**On weirdness**: abort

### Step 11: Force RLS on secret_t

**Action**: `docker exec pgpaw-jwt-pg psql -U postgres -c "ALTER TABLE secret_t FORCE ROW LEVEL SECURITY"`

**Observe**:
- Output is `ALTER TABLE`

**Awareness**:
- FORCE ensures the policy applies even to the table owner; confirm no error.

**On weirdness**: abort

### Step 12: Create the org-scoping policy

**Action**: `docker exec pgpaw-jwt-pg psql -U postgres -c "CREATE POLICY secret_by_org ON secret_t FOR SELECT TO member USING ( org_id = ((select current_setting('request.jwt.claims', true))::json->>'org_id')::int )"`

**Observe**:
- Output is `CREATE POLICY`

**Awareness**:
- The policy reads `request.jwt.claims` inline. A syntax error here leaves `secret_t` readable by no one (empty results), which could mask whether a token was accepted; confirm `CREATE POLICY` printed cleanly.

**On weirdness**: abort

### Step 13: Create the publication PgPaw replicates

**Action**: `docker exec pgpaw-jwt-pg psql -U postgres -c "CREATE PUBLICATION cache_server_pub FOR ALL TABLES"`

**Observe**:
- Output is `CREATE PUBLICATION`

**Awareness**:
- The name must be exactly `cache_server_pub` (PgPaw's default `--publication`). A typo means PgPaw replicates nothing and every query 400s as "not in publication".

**On weirdness**: abort

### Step 14: Launch the PRIMARY PgPaw with --jwt-secret (background)

**Action**: `cargo run --release -- serve --pg-host 127.0.0.1 --pg-port 55435 --pg-user postgres --pg-password postgres --pg-database postgres --data-dir /tmp/pgpaw-jwt-data --port 8083 --jwt-secret pgpaw-test-secret-please-change`

**Observe**:
- Process stays running (does not exit); startup logs show it connecting to the upstream and beginning replication
- A fresh `/tmp/pgpaw-jwt-data` directory appears

**Awareness**:
- Run this in the background and capture stdout/stderr to a log later Steps can tail. Watch for "JWT verification is not configured" or a key-parse error — that would mean `--jwt-secret` did not register and every private query would 401 regardless of token, masking the per-token distinctions this test depends on.
- Confirm the bind line shows `127.0.0.1:8083`, not a port picked up from a stale env var.
- The `run_in_background` wrapper exits while the server child stays alive. After launching, verify via `lsof -i :8083` — a present LISTEN line means it is running even if the wrapper reported completion.

**On weirdness**: abort

### Step 15: Poll the PRIMARY until the replica catches up (run N times)

**Action**: `curl -s http://127.0.0.1:8083/healthz`

**Observe**:
- Body is JSON with `"status":"ok"` and a numeric `watermark`
- Re-run this Step until `status` is `ok` with a non-zero watermark; a few `halted`/zero-watermark polls right after launch are expected

**Awareness**:
- If `status` stays `halted`, read the `reason` — a halted replica makes every `/query` return 503, NOT the 401 auth denials this test scores. Distinguish 503 (replica) from 401 (auth).
- Do not proceed to auth assertions until at least one `ok` poll has been seen, or `secret_t` may not be replicated yet.

**On weirdness**: retry once per poll; abort if still `halted` after several polls

### Step 16: Public query, NO token (baseline that public path works)

**Action**: `curl -s -i -X POST http://127.0.0.1:8083/query -H 'content-type: application/json' -d '{"sql":"select id, v from pub_t order by id"}'`

**Observe**:
- Status line is `303 See Other`
- A `Location:` header matches the shape `/q/{hash}/{version}`

**Awareness**:
- The 303 itself carries `Cache-Control: no-store` (the long-lived `public, max-age=259200` lives on the followed `/q/...` fetch, not on the redirect).
- A 401 here would mean `pub_t` was wrongly classified private — re-check the PUBLIC grant (Step 5) and that RLS is OFF on `pub_t`.

**On weirdness**: abort

### Step 17: Valid HS256 token over the private table (TOKEN_A) — must be ACCEPTED

**Action**: `curl -s -i -X POST http://127.0.0.1:8083/query -H 'content-type: application/json' -H 'authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoxLCJleHAiOjIwOTY5MDQ2MzF9.KGb25waEy8TTsaWqOzsENgQ8wU0EkyMjrCFiX3NHhDI' -d '{"sql":"select * from secret_t order by id"}'`

**Observe**:
- Status is `200 OK` with header `Cache-Control: private, no-store`
- Body is inline JSON (NOT a 303) containing ONLY org-1 rows (ids 11 and 12)

**Awareness**:
- This is the positive control: it proves the verifier accepts the ONE good shape, so any 401 in later Steps is a genuine rejection and not a broken-verifier artifact. If THIS returns 401, stop — the secret/token pairing or verifier registration is wrong and the whole rejection matrix is unjudgeable.
- Scan the body for any `org_id":2` — there should be none seeded, but its presence would signal an RLS leak.

**On weirdness**: abort

### Step 18: Expired token (TOKEN_EXPIRED) — must be 401

**Action**: `curl -s -i -X POST http://127.0.0.1:8083/query -H 'content-type: application/json' -H 'authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoxLCJleHAiOjE3ODE1NDEwMzF9.slXfdxqROE0gZm_IL73fHWiavjCT1Mf1daCL6XyIW24' -d '{"sql":"select * from secret_t order by id"}'`

**Observe**:
- Status is `401 Unauthorized`
- No `secret_t` rows appear in the body

**Awareness**:
- The signature is valid but `exp` is in the past; rejection must happen BEFORE the query runs. Any row content in the body means expiry is not being enforced.

**On weirdness**: abort

### Step 19: Bad-signature token (TOKEN_BADSIG) — must be 401

**Action**: `curl -s -i -X POST http://127.0.0.1:8083/query -H 'content-type: application/json' -H 'authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoxLCJleHAiOjIwOTY5MDQ2MzF9.3Q9qFFqMZBsC4sNP4Nj5SM_-eLuEJ-iO0LOqSDVWniU' -d '{"sql":"select * from secret_t order by id"}'`

**Observe**:
- Status is `401 Unauthorized`
- No `secret_t` rows appear in the body

**Awareness**:
- Same claims as TOKEN_A but signed with the wrong secret. A 200 here is a catastrophic bypass — the signature is not being checked.

**On weirdness**: abort

### Step 20: alg:none forgery (ALG_NONE) — must be 401 (security-critical)

**Action**: `curl -s -i -X POST http://127.0.0.1:8083/query -H 'content-type: application/json' -H 'authorization: Bearer eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoxLCJleHAiOjIwOTY5MDQ2MzF9.' -d '{"sql":"select * from secret_t order by id"}'`

**Observe**:
- Status is `401 Unauthorized`
- No `secret_t` rows appear in the body

**Awareness**:
- This is the load-bearing security assertion. The header declares `alg:none` and the signature segment is empty; the server is alg-pinned to HS256 and MUST refuse it. A `200` here is a total auth bypass (algorithm-downgrade attack succeeded) — record it as a critical finding, not a soft note.
- Note the trailing dot in the token (empty third segment) is intentional and must be sent as-is.

**On weirdness**: abort

### Step 21: Missing-role token (MISSING_ROLE) — must be 401

**Action**: `curl -s -i -X POST http://127.0.0.1:8083/query -H 'content-type: application/json' -H 'authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJvcmdfaWQiOjEsImV4cCI6MjA5NjkwNDYzMX0.62S2Y-E6-PQHKUJoitNKI1QSda5fMJUdiFxo594a-b0' -d '{"sql":"select * from secret_t order by id"}'`

**Observe**:
- Status is `401 Unauthorized`
- The error envelope conveys the token lacks the `role` claim (wording may vary; meaning = no string role claim)

**Awareness**:
- This token is validly signed and unexpired — the ONLY defect is a missing `role`. A 200 here would mean the verifier accepts a token it cannot map to a Postgres role. Judge the message by meaning ("missing role"), not exact bytes.

**On weirdness**: abort

### Step 22: Raw token with NO Bearer prefix — must be 401 malformed

**Action**: `curl -s -i -X POST http://127.0.0.1:8083/query -H 'content-type: application/json' -H 'authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoxLCJleHAiOjIwOTY5MDQ2MzF9.KGb25waEy8TTsaWqOzsENgQ8wU0EkyMjrCFiX3NHhDI' -d '{"sql":"select * from secret_t order by id"}'`

**Observe**:
- Status is `401 Unauthorized`
- The error envelope conveys a malformed Authorization header (meaning = header is not a `Bearer <token>`)

**Awareness**:
- This is TOKEN_A's exact bytes but WITHOUT the `Bearer ` prefix. Even though the underlying token is valid, the header parser must reject it before verification — a 200 here means header parsing is too lenient. Confirm no rows leak.

**On weirdness**: abort

### Step 23: Wrong auth scheme (Basic) — must be 401 malformed

**Action**: `curl -s -i -X POST http://127.0.0.1:8083/query -H 'content-type: application/json' -H 'authorization: Basic dXNlcjpwYXNz' -d '{"sql":"select * from secret_t order by id"}'`

**Observe**:
- Status is `401 Unauthorized`
- The error envelope conveys a malformed Authorization header (not a Bearer token)

**Awareness**:
- `Basic dXNlcjpwYXNz` is a valid base64 credential but the wrong scheme. The server must not attempt to verify it as a JWT; expect the same malformed-header rejection as Step 22. No rows.

**On weirdness**: abort

### Step 24: Public query is unaffected by auth on the jwt-configured server

**Action**: `curl -s -i -X POST http://127.0.0.1:8083/query -H 'content-type: application/json' -d '{"sql":"select * from pub_t order by id"}'`

**Observe**:
- Status line is `303 See Other`
- A `Location:` header matches the shape `/q/{hash}/{version}`

**Awareness**:
- Configuring a verifier must NOT make public reads require a token. A 401 here would mean the auth gate over-reached onto the public path. This mirrors Step 16 and confirms the public classification survived all the private rejections.

**On weirdness**: abort

### Step 25: Launch the SECOND PgPaw with NO jwt flags (background)

**Action**: `cargo run --release -- serve --pg-host 127.0.0.1 --pg-port 55435 --pg-user postgres --pg-password postgres --pg-database postgres --data-dir /tmp/pgpaw-jwt-data-noauth --port 8084`

**Observe**:
- Process stays running; startup logs show no JWT verifier configured and replication beginning
- A fresh `/tmp/pgpaw-jwt-data-noauth` directory appears

**Awareness**:
- This server intentionally has NO `--jwt-secret`/`--jwt-public-key`/`--jwt-jwks-url`. It points at the SAME Postgres container (port 55435) but a SEPARATE data-dir, so it builds its own replica. Confirm the bind line shows `127.0.0.1:8084` and does not collide with the primary on 8083.
- Verify via `lsof -i :8084` that it is actually listening even if the background wrapper reported completion.

**On weirdness**: abort

### Step 26: Poll the no-jwt server until its replica catches up (run N times)

**Action**: `curl -s http://127.0.0.1:8084/healthz`

**Observe**:
- Body is JSON with `"status":"ok"` and a numeric non-zero `watermark`
- Re-run until `ok`; early `halted`/zero-watermark polls are expected

**Awareness**:
- This is a SECOND independent replica; it may take its own moment to catch up. Do not run Steps 27-28 until at least one `ok` poll — otherwise a 503 (replica) could be mistaken for the 401 / 303 outcomes those steps expect.

**On weirdness**: retry once per poll; abort if still `halted` after several polls

### Step 27: Present TOKEN_A to the no-jwt server over the private table — must be 401 not-configured

**Action**: `curl -s -i -X POST http://127.0.0.1:8084/query -H 'content-type: application/json' -H 'authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoxLCJleHAiOjIwOTY5MDQ2MzF9.KGb25waEy8TTsaWqOzsENgQ8wU0EkyMjrCFiX3NHhDI' -d '{"sql":"select * from secret_t order by id"}'`

**Observe**:
- Status is `401 Unauthorized`
- The error envelope conveys that JWT verification is not configured (meaning = a token was presented but the server cannot verify)
- No `secret_t` rows appear

**Awareness**:
- This is the fail-closed assertion: a token offered to a server with no verifier must be REFUSED, never trusted. A `200` with rows here is a fail-open bug — the server ran a private query off an unverified token. The token bytes are TOKEN_A (valid against the primary), proving the difference is purely the missing verifier config.

**On weirdness**: abort

### Step 28: No-jwt server public query, NO token — must still be 303

**Action**: `curl -s -i -X POST http://127.0.0.1:8084/query -H 'content-type: application/json' -d '{"sql":"select * from pub_t order by id"}'`

**Observe**:
- Status line is `303 See Other`
- A `Location:` header matches the shape `/q/{hash}/{version}`

**Awareness**:
- Refusing tokens must not break the public path. A server with no auth config still serves public, token-free reads. A 401 / 503 here would mean the no-jwt server is broken for the legitimate anonymous case.

**On weirdness**: abort

### Step 29: Launch attempt with --jwt-jwks-url (foreground, expect fast non-zero exit)

**Action**: `cargo run --release -- serve --pg-host 127.0.0.1 --pg-port 55435 --pg-user postgres --pg-password postgres --pg-database postgres --data-dir /tmp/pgpaw-jwt-data-jwks --port 8085 --jwt-jwks-url https://example.test/jwks`

**Observe**:
- The process exits NON-ZERO promptly (it does not stay running and does not begin serving)
- stderr carries a Config error whose meaning is that JWKS URL verification is not yet implemented (wording may vary; expect mention of "not yet implemented" and a pointer to `--jwt-secret`/`--jwt-public-key`)

**Awareness**:
- Run this in the FOREGROUND so the exit is observable; do NOT background it and do NOT leave it running. The unimplemented path must fail LOUD at startup, not silently start a server that then accepts tokens. If it instead begins listening on 8085, that is the failure mode — kill it and record it.
- Because it should exit before binding, port 8085 and the `/tmp/pgpaw-jwt-data-jwks` dir should remain effectively unused; if the dir was created, remove it during cleanup.

**On weirdness**: abort

## Expected Behavior

- Exactly one token shape is accepted on the jwt-configured PRIMARY: a correctly-signed, unexpired HS256 token carrying a string `role` claim (TOKEN_A) → `200` inline JSON with `Cache-Control: private, no-store` and ONLY org-1 rows (ids 11, 12).
- Every other token shape over the private table is `401 Unauthorized` with no `secret_t` rows in the body: expired (TOKEN_EXPIRED), bad-signature (TOKEN_BADSIG), `alg:none` forgery (ALG_NONE), missing-role (MISSING_ROLE), raw token with no `Bearer ` prefix, and the `Basic` scheme.
- The `alg:none` token is rejected specifically because the verifier is alg-pinned to HS256; alg-pinning defeats the algorithm-downgrade attack. A `200` on ALG_NONE is a total auth bypass.
- The missing-role rejection's message conveys the token lacks the `role` claim; the no-Bearer and wrong-scheme rejections convey a malformed Authorization header. Judge these by meaning, not exact wording.
- Public queries (`pub_t`) never require a token, on BOTH the jwt-configured server and the no-jwt server: each returns `303 See Other` with a `Location` of shape `/q/{hash}/{version}`.
- A server launched with NO jwt config refuses any presented token over the private table with `401` whose meaning is "JWT verification is not configured" (fail-closed), while still serving public queries `303`.
- `--jwt-jwks-url` is rejected at startup: the process exits non-zero with a Config error meaning "JWKS URL verification is not yet implemented" and never begins serving (fails loud, not silently accepting tokens).
- KNOWN COVERAGE GAP: RS256/ES256 PEM verification (via `--jwt-public-key`) and live algorithm-confusion against a real RSA public key are NOT exercised here — minting asymmetric tokens needs crypto tooling (openssl/python/node) the harness does not have. The `alg:none` forgery covers the algorithm-downgrade defense without asymmetric keys; the PEM acceptance path remains untested by this notebook.

Reserve exact-match only for system-composed artifacts: the status codes (`303`, `200`, `401`), the header strings (`Cache-Control: private, no-store`, `Cache-Control: no-store`), and the `/q/{hash}/{version}` Location shape. Token-rejection wording is judged by meaning, not byte-for-byte.

## Fail Modes

- **alg:none (Step 20) or bad-sig (Step 19) returns `200`** — catastrophic auth bypass; the verifier is not alg-pinned or the signature is not being checked → confirm `--jwt-secret` registered an HS256 verifier (Step 14 logs) and that `pinned(Algorithm::HS256)` is in effect; treat as a critical finding, not a flake.
- **No-Bearer raw token (Step 22) or `Basic` scheme (Step 23) accepted** — header parsing too lenient; the `strip_prefix("Bearer ")` gate is being bypassed → verify the response is the malformed-header 401 and that no rows leaked.
- **No-jwt server trusts TOKEN_A (Step 27 returns `200` with rows)** — fail-open; verifier presence not gated. The server ran a private query off an unverified token → confirm Step 25 launched with NO jwt flags and that the "not configured" branch fires.
- **`--jwt-jwks-url` launch (Step 29) silently starts and listens on 8085** — the unimplemented path is failing open instead of closed → it must exit non-zero at startup; kill any listener and record the bypass.
- **Valid token (Step 17) yields `401`** — `--jwt-secret` mismatch or verifier not configured → confirm the secret in Step 14 matches `pgpaw-test-secret-please-change` and startup logs do not say "JWT verification is not configured". Until this passes, the whole rejection matrix is unjudgeable.
- **All queries `503`** — replica halted or never caught up → re-read the relevant `/healthz` `reason`; this is a replication failure distinct from auth, so do not score auth steps until that server is `ok`.
- **Public query `400` "not in publication"** — publication name mismatch → confirm the publication is exactly `cache_server_pub` and includes the tables (`\dRp+` in the container).

## Cleanup

### Cleanup 1: Stop and remove the Postgres container

**Action**: `docker rm -f pgpaw-jwt-pg`

**Observe**:
- Output prints the container name; `docker ps -a --filter name=pgpaw-jwt-pg` is then empty

**Awareness**:
- A leftover container holds port 55435 and would break the next fresh run.

**On weirdness**: note-and-continue

### Cleanup 2: Stop BOTH PgPaw server processes

**Action**: `pkill -f '/tmp/pgpaw-jwt-data'`

**Observe**:
- Both background PgPaw processes exit; ports 8083 and 8084 are freed (`lsof -i :8083` and `lsof -i :8084` print nothing)

**Awareness**:
- The match is by the unique data-dir path prefix `/tmp/pgpaw-jwt-data`, which matches BOTH the primary (`/tmp/pgpaw-jwt-data`) and the no-auth server (`/tmp/pgpaw-jwt-data-noauth`). The process command line is `... serve ...`, so killing by the data-dir path is the reliable unique key. Confirm BOTH server processes actually died — a stray one keeps its data-dir locked.
- If a socket lingers in `CLOSE_WAIT` after pkill, use `kill -9 <PID>` to force-terminate.

**On weirdness**: note-and-continue

### Cleanup 3: Remove the primary replica data directory

**Action**: `rm -rf /tmp/pgpaw-jwt-data`

**Observe**:
- `/tmp/pgpaw-jwt-data` no longer exists (`test -d /tmp/pgpaw-jwt-data` returns non-zero)

**Awareness**:
- Reusing this dir on a later run reuses a stale replica and violates the FRESH WORKSPACE contract.

**On weirdness**: note-and-continue

### Cleanup 4: Remove the no-auth replica data directory

**Action**: `rm -rf /tmp/pgpaw-jwt-data-noauth`

**Observe**:
- `/tmp/pgpaw-jwt-data-noauth` no longer exists (`test -d /tmp/pgpaw-jwt-data-noauth` returns non-zero)

**Awareness**:
- The `--jwt-jwks-url` attempt in Step 29 may have created `/tmp/pgpaw-jwt-data-jwks` before failing; if `test -d /tmp/pgpaw-jwt-data-jwks` returns zero, remove it too in a follow-up so it does not leak into a later run.

**On weirdness**: note-and-continue
