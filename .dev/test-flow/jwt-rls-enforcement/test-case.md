---
id: jwt-rls-enforcement
title: JWT-scoped /query enforces upstream RLS so each tenant sees only its own rows, with correct 303/200/401/403 status handling
tier: live
component: auth
target: pgpaw (cache-server binary)
prerequisites:
  - "FRESH WORKSPACE REQUIRED — every live test MUST run against a freshly-created workspace (do NOT reuse a previously-tested one). A stale --data-dir replica or a leftover docker container contaminates RLS observations."
  - "`docker --version` returns a version string"
  - "`curl --version` returns a version string"
  - "`cargo --version` returns a version string (PgPaw built from this repo, or `pgpaw` on PATH)"
  - "TCP port 55432 on 127.0.0.1 is free (`lsof -i :55432` prints nothing)"
  - "TCP port 8080 on 127.0.0.1 is free (`lsof -i :8080` prints nothing)"
expected_duration_secs: 420
tags: [jwt, rls, auth, multi-tenant, access-control, live]
priority: high
created: 2026-06-16
author: senior-qa
---

## Objective

Verify that a JWT-authenticated `POST /query` runs under the token's Postgres role so that `SELECT *` and JOIN reads over an RLS-protected table return only the calling tenant's rows, while public reads stay token-free and cacheable, and missing/invalid tokens and private live streams are rejected with the documented status codes.

## Preconditions

- `docker ps` succeeds (daemon reachable)
- The fixed JWT secret below (`pgpaw-test-secret-please-change`) will be passed to PgPaw via `--jwt-secret`; the tokens in Inputs are pre-signed against it
- No container named `pgpaw-rls-pg` already exists (`docker ps -a --filter name=pgpaw-rls-pg --format '{{.Names}}'` prints nothing)
- The data dir `/tmp/pgpaw-rls-data` does not yet exist (`test -d /tmp/pgpaw-rls-data` returns non-zero)

## Inputs

Fixed HS256 secret PgPaw is launched with:

```text
pgpaw-test-secret-please-change
```

Pre-signed test tokens (HS256, `role=member`, long-lived `exp`). Tenant is carried in `org_id`:

```text
# Tenant A — org_id = 1
TOKEN_A=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoxLCJleHAiOjIwOTY5MDQ2MzF9.KGb25waEy8TTsaWqOzsENgQ8wU0EkyMjrCFiX3NHhDI

# Tenant B — org_id = 2
TOKEN_B=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoyLCJleHAiOjIwOTY5MDQ2MzF9.qxynUpmoxK4Bp4KvBXA0BVIgbLaqR3yBcW0XOsVcfis

# Expired (exp in the past) — for the 401 invalid case
TOKEN_EXPIRED=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoxLCJleHAiOjE3ODE1NDEwMzF9.slXfdxqROE0gZm_IL73fHWiavjCT1Mf1daCL6XyIW24

# Bad signature (A's claims signed with the wrong secret) — alternate 401 case
TOKEN_BADSIG=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoxLCJleHAiOjIwOTY5MDQ2MzF9.3Q9qFFqMZBsC4sNP4Nj5SM_-eLuEJ-iO0LOqSDVWniU
```

Schema + seed (multi-tenant, RLS, login role) — applied one statement per Step in `## Steps`:

```sql
-- orgs (public-readable, RLS off)
CREATE TABLE orgs (id int PRIMARY KEY, name text);
INSERT INTO orgs VALUES (1,'Acme'), (2,'Globex');

-- documents (RLS on, tenant-scoped by org_id)
CREATE TABLE documents (id int PRIMARY KEY, org_id int REFERENCES orgs(id), title text);
INSERT INTO documents VALUES
  (101,1,'A-doc-one'), (102,1,'A-doc-two'),
  (201,2,'B-doc-one'), (202,2,'B-doc-two'), (203,2,'B-doc-three');

-- non-superuser login role the JWT role claim maps to
CREATE ROLE member LOGIN;
GRANT SELECT ON orgs TO PUBLIC;
GRANT SELECT ON documents TO member;

-- RLS: a member sees only documents whose org_id matches their JWT org_id claim
ALTER TABLE documents ENABLE ROW LEVEL SECURITY;
ALTER TABLE documents FORCE ROW LEVEL SECURITY;
CREATE POLICY documents_by_org ON documents FOR SELECT TO member
  USING ( org_id = ((select current_setting('request.jwt.claims', true))::json->>'org_id')::int );

-- publication PgPaw replicates
CREATE PUBLICATION cache_server_pub FOR ALL TABLES;
```

Tenant partition (ground truth the tester judges row results against):

```text
org_id=1 (Tenant A) owns document ids: 101, 102
org_id=2 (Tenant B) owns document ids: 201, 202, 203
```

## Steps

> **Notebook discipline**: Each Step is ONE action. The tester runs it, observes, judges, then moves to the next. No `&&`, `;`, multi-line shells, or for-loops inside a Step's Action. Polling = a Step the tester runs N times.

### Step 1: Provision a fresh logical-replication Postgres

**Action**: `docker run -d --name pgpaw-rls-pg -e POSTGRES_PASSWORD=postgres -p 55432:5432 postgres:16 -c wal_level=logical`

**Observe**:
- Command exits 0 and prints a 64-hex container id
- `docker ps --filter name=pgpaw-rls-pg` shows the container as Up

**Awareness**:
- This pulls `postgres:16` on first run — a slow pull is normal, not a failure. A non-zero exit citing "port is already allocated" means port 55432 was not actually free; abort and re-check the precondition.
- Confirm no OTHER postgres container is bound to 55432 that could shadow this one.

**On weirdness**: abort

### Step 2: Confirm wal_level is logical

**Action**: `docker exec pgpaw-rls-pg psql -U postgres -tAc "SHOW wal_level"`

**Observe**:
- Output is exactly `logical`

**Awareness**:
- If this prints `replica` the `-c wal_level=logical` flag did not take — logical replication will silently never start downstream and every private query will look empty. Treat a wrong value as a hard stop.
- A "FATAL: the database system is starting up" error means Postgres is not ready yet; wait a few seconds and retry.

**On weirdness**: retry once (for startup race); abort if value is not `logical`

### Step 3: Create the orgs table

**Action**: `docker exec pgpaw-rls-pg psql -U postgres -c "CREATE TABLE orgs (id int PRIMARY KEY, name text)"`

**Observe**:
- Output is `CREATE TABLE`

**Awareness**:
- Watch for any NOTICE about an existing relation — that would mean the container is not actually fresh.

**On weirdness**: abort

### Step 4: Seed the orgs rows

**Action**: `docker exec pgpaw-rls-pg psql -U postgres -c "INSERT INTO orgs VALUES (1,'Acme'),(2,'Globex')"`

**Observe**:
- Output is `INSERT 0 2`

**Awareness**:
- A row count other than 2 means partial seed; downstream public-query assertions become unreliable.

**On weirdness**: abort

### Step 5: Create the documents table

**Action**: `docker exec pgpaw-rls-pg psql -U postgres -c "CREATE TABLE documents (id int PRIMARY KEY, org_id int REFERENCES orgs(id), title text)"`

**Observe**:
- Output is `CREATE TABLE`

**Awareness**:
- The FK to `orgs` must succeed; a missing-relation error means Step 3 silently failed.

**On weirdness**: abort

### Step 6: Seed the documents rows (both tenants)

**Action**: `docker exec pgpaw-rls-pg psql -U postgres -c "INSERT INTO documents VALUES (101,1,'A-doc-one'),(102,1,'A-doc-two'),(201,2,'B-doc-one'),(202,2,'B-doc-two'),(203,2,'B-doc-three')"`

**Observe**:
- Output is `INSERT 0 5`

**Awareness**:
- This is the overlapping cross-tenant dataset the whole test hinges on — 2 rows for org 1, 3 rows for org 2. If the count is not 5, the isolation assertion later is meaningless.

**On weirdness**: abort

### Step 7: Create the non-superuser login role

**Action**: `docker exec pgpaw-rls-pg psql -U postgres -c "CREATE ROLE member LOGIN"`

**Observe**:
- Output is `CREATE ROLE`

**Awareness**:
- `member` must NOT be a superuser and must NOT have BYPASSRLS — either would make RLS a no-op and let a tenant see everything. A "role already exists" notice means a non-fresh container.

**On weirdness**: abort

### Step 8: Grant public read on orgs

**Action**: `docker exec pgpaw-rls-pg psql -U postgres -c "GRANT SELECT ON orgs TO PUBLIC"`

**Observe**:
- Output is `GRANT`

**Awareness**:
- This is what makes an `orgs`-only query classify PUBLIC (no token needed). If it is missed, the later no-token public query will wrongly demand a token.

**On weirdness**: abort

### Step 9: Grant member read on documents

**Action**: `docker exec pgpaw-rls-pg psql -U postgres -c "GRANT SELECT ON documents TO member"`

**Observe**:
- Output is `GRANT`

**Awareness**:
- Without this grant, even an authenticated member hits a privilege error — which would surface later as a 403, not the empty/scoped 200 we expect.

**On weirdness**: abort

### Step 10: Enable RLS on documents

**Action**: `docker exec pgpaw-rls-pg psql -U postgres -c "ALTER TABLE documents ENABLE ROW LEVEL SECURITY"`

**Observe**:
- Output is `ALTER TABLE`

**Awareness**:
- Enabling RLS is exactly what flips `documents` from public to access-controlled in PgPaw's classifier. After this, a no-token query touching `documents` must 401.

**On weirdness**: abort

### Step 11: Force RLS on documents

**Action**: `docker exec pgpaw-rls-pg psql -U postgres -c "ALTER TABLE documents FORCE ROW LEVEL SECURITY"`

**Observe**:
- Output is `ALTER TABLE`

**Awareness**:
- FORCE ensures the policy applies even to the table owner; without it an owning role could bypass the filter. Confirm no error.

**On weirdness**: abort

### Step 12: Create the tenant-scoping policy

**Action**: `docker exec pgpaw-rls-pg psql -U postgres -c "CREATE POLICY documents_by_org ON documents FOR SELECT TO member USING ( org_id = ((select current_setting('request.jwt.claims', true))::json->>'org_id')::int )"`

**Observe**:
- Output is `CREATE POLICY`

**Awareness**:
- The policy reads `request.jwt.claims` inline (no helper function), matching the README guidance — helper functions are not replicated into the cache. A syntax error here leaves `documents` readable by no one (empty results for every tenant).

**On weirdness**: abort

### Step 13: Create the publication PgPaw replicates

**Action**: `docker exec pgpaw-rls-pg psql -U postgres -c "CREATE PUBLICATION cache_server_pub FOR ALL TABLES"`

**Observe**:
- Output is `CREATE PUBLICATION`

**Awareness**:
- The name must be exactly `cache_server_pub` (PgPaw's default `--publication`). A typo means PgPaw replicates nothing and every query 400s as "not in publication".

**On weirdness**: abort

### Step 14: Launch PgPaw against the temp Postgres (background)

**Action**: `cargo run --release -- serve --pg-host 127.0.0.1 --pg-port 55432 --pg-user postgres --pg-password postgres --pg-database postgres --data-dir /tmp/pgpaw-rls-data --port 8080 --jwt-secret pgpaw-test-secret-please-change`

**Observe**:
- Process stays running (does not exit); startup logs show it connecting to the upstream and beginning replication
- A fresh `/tmp/pgpaw-rls-data` directory appears

**Awareness**:
- Run this in the background and capture stdout/stderr to a log the later Steps can tail. Watch for "JWT verification is not configured" or a key-parse error — that would mean `--jwt-secret` did not register and every private query would 401 regardless of token.
- Confirm the bind line shows `127.0.0.1:8080`, not a different port picked up from a stale env var.
- The `run_in_background` tool's shell wrapper exits while the server child process stays alive. After launching, verify the server is running via `ps -p <PID>` and `lsof -i :8080` — a missing LISTEN line means the process truly exited; a present LISTEN line means it is running even if the shell wrapper reported completion.

**On weirdness**: abort

### Step 15: Poll until the replica catches up (run N times)

**Action**: `curl -s http://127.0.0.1:8080/healthz`

**Observe**:
- Body is JSON with `"status":"ok"` and a numeric `watermark`
- Re-run this Step until `status` is `ok` with a non-zero watermark (the replica has applied the seed); a few `halted`/zero-watermark polls right after launch are expected

**Awareness**:
- If `status` stays `halted`, read the `reason` — a halted replica makes every `/query` return 503, which is NOT the same as the RLS denials this test cares about. Distinguish 503 (replica) from 401/403 (auth).
- Do not proceed to row-content assertions until at least one `ok` poll has been seen, or the documents table may not be replicated yet.

**On weirdness**: retry once per poll; abort if still `halted` after several polls

### Step 16: Public query with NO Authorization header

**Action**: `curl -s -i -X POST http://127.0.0.1:8080/query -H 'content-type: application/json' -d '{"sql":"select id, name from orgs order by id"}'`

**Observe**:
- Status line is `303 See Other`
- A `Location:` header matches the shape `/q/{hash}/{version}` (hash and version are opaque; only the shape is fixed)

**Awareness**:
- The 303 response itself carries `Cache-Control: no-store` (the long-lived `public, max-age=259200` lives on the followed `/q/...` fetch, not on the redirect). Note the exact `Location` value to follow it in the next Step.
- A 401 here would mean `orgs` was wrongly classified private — re-check the PUBLIC grant (Step 8) and that RLS is OFF on `orgs`.

**On weirdness**: abort

### Step 17: Follow the public snapshot URL

**Action**: `curl -s -i http://127.0.0.1:8080/q/<hash>/<version>` (substitute the Location captured in Step 16)

**Observe**:
- Status is `200 OK`
- Header includes `Cache-Control: public, max-age=259200` and an `ETag`
- Body is a JSON array of both orgs (Acme and Globex)

**Awareness**:
- This confirms the public path is genuinely CDN-cacheable. A `private, no-store` here would mean the public/private classifier mis-tagged the row.

**On weirdness**: note-and-continue

### Step 18: SELECT * over the RLS table with Tenant A's token

**Action**: `curl -s -i -X POST http://127.0.0.1:8080/query -H 'content-type: application/json' -H 'authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoxLCJleHAiOjIwOTY5MDQ2MzF9.KGb25waEy8TTsaWqOzsENgQ8wU0EkyMjrCFiX3NHhDI' -d '{"sql":"select * from documents order by id"}'`

**Observe**:
- Status is `200 OK` with header `Cache-Control: private, no-store`
- Body is inline JSON (NOT a 303 redirect)
- Body contains ONLY Tenant A's documents (ids 101 and 102); it contains ZERO Tenant-B rows (no 201/202/203)

**Awareness**:
- This is the load-bearing security assertion. Scan the body for any `org_id":2` or B-doc title — its presence is a critical leak even if the count looks right.
- A `private, no-store` header AND inline body together prove the access-controlled path was taken (never the cache). If you see a 303, RLS classification failed.

**On weirdness**: abort

### Step 19: JOIN across orgs+documents with Tenant A's token

**Action**: `curl -s -i -X POST http://127.0.0.1:8080/query -H 'content-type: application/json' -H 'authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoxLCJleHAiOjIwOTY5MDQ2MzF9.KGb25waEy8TTsaWqOzsENgQ8wU0EkyMjrCFiX3NHhDI' -d '{"sql":"select d.id, d.title, o.name from documents d join orgs o on o.id = d.org_id order by d.id"}'`

**Observe**:
- Status is `200 OK` with `Cache-Control: private, no-store`
- Joined rows reference ONLY Acme (org 1) and document ids 101/102; no Globex/B rows appear despite `orgs` being public

**Awareness**:
- Because the query touches `documents` (RLS on), the WHOLE join must be access-controlled and tenant-filtered — joining in a public table must NOT widen the result back to B's rows.
- Confirm the response is still inline JSON and not a 303 — a single private table must taint the whole join.

**On weirdness**: abort

### Step 20: Same SELECT * with Tenant B's token (cross-check)

**Action**: `curl -s -i -X POST http://127.0.0.1:8080/query -H 'content-type: application/json' -H 'authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoyLCJleHAiOjIwOTY5MDQ2MzF9.qxynUpmoxK4Bp4KvBXA0BVIgbLaqR3yBcW0XOsVcfis' -d '{"sql":"select * from documents order by id"}'`

**Observe**:
- Status is `200 OK` with `Cache-Control: private, no-store`
- Body contains ONLY Tenant B's documents (ids 201, 202, 203); ZERO Tenant-A rows (no 101/102)
- The row set is disjoint from Step 18's result — no overlap at all

**Awareness**:
- Same role (`member`), different `org_id` claim, different result set — this proves the filter keys on the per-request claim, not on a cached or process-wide value. If B sees A's rows (or vice versa), the claim is not being applied per request.

**On weirdness**: abort

### Step 21: Private query with NO token

**Action**: `curl -s -i -X POST http://127.0.0.1:8080/query -H 'content-type: application/json' -d '{"sql":"select * from documents order by id"}'`

**Observe**:
- Status is `401 Unauthorized`
- Error envelope conveys that a bearer token is required (wording may vary; meaning must be "auth required")

**Awareness**:
- It must be `401`, NOT a `200` with zero rows. A zero-row 200 would mean PgPaw ran the private query anonymously (a fail-open bug) instead of refusing it.

**On weirdness**: abort

### Step 22: Private query with an expired/invalid token

**Action**: `curl -s -i -X POST http://127.0.0.1:8080/query -H 'content-type: application/json' -H 'authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoxLCJleHAiOjE3ODE1NDEwMzF9.slXfdxqROE0gZm_IL73fHWiavjCT1Mf1daCL6XyIW24' -d '{"sql":"select * from documents order by id"}'`

**Observe**:
- Status is `401 Unauthorized`
- No document rows appear in the body

**Awareness**:
- An expired/invalid token must be rejected BEFORE the query runs. If any row content leaks in the body, verification is being skipped. (To distinguish expiry from signature failure, the tester may optionally re-run with TOKEN_BADSIG from Inputs and expect the same 401.)

**On weirdness**: abort

### Step 23: Public live stream works (SSE)

**Action**: `curl -s -N -i -X POST 'http://127.0.0.1:8080/query?live=true' -H 'content-type: application/json' -d '{"sql":"select id, name from orgs order by id"}'`

**Observe**:
- Status is `200 OK` with `Content-Type: text/event-stream`
- The stream opens and emits at least a first `data:` event (a snapshot pointer); the connection stays open

**Awareness**:
- This is a streaming endpoint — the tester must cap the read (timeout / Ctrl-C after the first event) so the Step does not hang indefinitely. Capturing the first event is sufficient.
- Confirm the content type is the event-stream type, not `application/json` — the latter would mean live mode was ignored.

**On weirdness**: note-and-continue

### Step 24: Private live stream is rejected

**Action**: `curl -s -i -X POST 'http://127.0.0.1:8080/query?live=true' -H 'content-type: application/json' -H 'authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoxLCJleHAiOjIwOTY5MDQ2MzF9.KGb25waEy8TTsaWqOzsENgQ8wU0EkyMjrCFiX3NHhDI' -d '{"sql":"select * from documents order by id"}'`

**Observe**:
- Status is `403 Forbidden`
- The body is an error envelope (no SSE stream opens, no document rows)

**Awareness**:
- Even with a VALID token, a live request over an access-controlled table must be refused with `403` — not `200` and not a stream. A `200`/event-stream here would mean private live deltas leaked, which is out of scope and unsafe.

**On weirdness**: abort

## Expected Behavior

- A query touching only `orgs` (RLS off, granted to PUBLIC) is served WITHOUT a token as a `303 See Other` whose `Location` matches `/q/{hash}/{version}`; following that URL returns `200` with `Cache-Control: public, max-age=259200` and an `ETag`. No token is ever required for it.
- A query touching `documents` (RLS on) is access-controlled: with a valid token it returns `200` inline JSON with `Cache-Control: private, no-store`, never a `303`, and is never written to the shared cache.
- With Tenant A's token the document results contain ONLY A's rows (org 1) and zero B rows; with Tenant B's token they contain ONLY B's rows (org 2) and zero A rows. The two result sets are disjoint. This holds for `SELECT *` and for a JOIN that also reads the public `orgs` table — the private table taints the whole statement.
- A private query with NO token is rejected `401` (not a zero-row `200`); an expired or bad-signature token is also `401`, with no row content in the body.
- Public live (`?live=true` over `orgs`) opens a `text/event-stream` and emits at least a snapshot event; private live (`?live=true` over `documents`) is rejected `403` even with a valid token.
- Throughout, `/healthz` reports `status: ok` with a monotonic watermark; the role used (`member`) is a non-superuser without BYPASSRLS, so Postgres itself — not PgPaw — is the row-filtering authority.

Reserve exact-match only for system-composed artifacts: the status codes (`303`, `200`, `401`, `403`), the header strings (`Cache-Control: private, no-store`, `Cache-Control: public, max-age=259200`, `Content-Type: text/event-stream`), and the `/q/{hash}/{version}` Location shape. Row counts/content are judged against the seeded tenant partition (A: 101,102 / B: 201,202,203), not by exact byte match of the JSON.

## Fail Modes

- **Every private query returns empty `200` for all tenants** — RLS policy syntax wrong, or claims not forwarded → check `documents_by_org` exists (`\d+ documents` in the container) and that PgPaw logs show `request.jwt.claims` being set; confirm `wal_level=logical` actually replicated the policy.
- **Tenant A sees Tenant B's rows (leak)** — role is superuser/BYPASSRLS, or `query_as` is not issuing `SET LOCAL ROLE`/claims per request → verify `member` privileges (`\du member`), and that the response carried `private, no-store` (proving the access-controlled path, not a cached snapshot, was hit).
- **No-token private query returns `200` with zero rows instead of `401`** — fail-open classification bug → check PgPaw classified `documents` as access-controlled (RLS-enabled) and that the `--jwt-secret` registered a verifier (logs).
- **Valid token still yields `401`** — `--jwt-secret` mismatch or verifier not configured → confirm the secret passed in Step 14 matches the one the tokens were signed with (`pgpaw-test-secret-please-change`) and that startup logs do not say "JWT verification is not configured".
- **All queries `503`** — replica halted or never caught up → re-read `/healthz` `reason`; this is a replication failure, distinct from any auth behavior, so do not score auth steps until `/healthz` is `ok`.
- **Public query `400` "not in publication"** — publication name mismatch → confirm the publication is exactly `cache_server_pub` and includes the tables (`\dRp+` in the container).

## Cleanup

### Cleanup 1: Stop and remove the Postgres container

**Action**: `docker rm -f pgpaw-rls-pg`

**Observe**:
- Output prints the container name; `docker ps -a --filter name=pgpaw-rls-pg` is then empty

**Awareness**:
- A leftover container holds port 55432 and would break the next fresh run.

**On weirdness**: note-and-continue

### Cleanup 2: Stop the PgPaw server process

**Action**: `pkill -f 'pgpaw serve'`

**Observe**:
- The background PgPaw process exits; port 8080 is freed (`lsof -i :8080` prints nothing)

**Awareness**:
- If multiple pgpaw processes were running, confirm all are gone — a stray one keeps the data-dir locked.
- If the socket enters `CLOSE_WAIT` after pkill, use `kill -9 <PID>` to force-terminate and free the port.

**On weirdness**: note-and-continue

### Cleanup 3: Remove the replica data directory

**Action**: `rm -rf /tmp/pgpaw-rls-data`

**Observe**:
- `/tmp/pgpaw-rls-data` no longer exists (`test -d /tmp/pgpaw-rls-data` returns non-zero)

**Awareness**:
- Reusing this dir on a later run reuses a stale replica and violates the FRESH WORKSPACE contract.

**On weirdness**: note-and-continue
