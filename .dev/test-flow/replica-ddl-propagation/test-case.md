---
id: replica-ddl-propagation
title: Security DDL applied upstream AFTER launch propagates into the replica and re-classifies a table from public to private with no PgPaw restart
tier: live
component: replica
target: pgpaw (cache-server binary)
prerequisites:
  - "FRESH WORKSPACE REQUIRED — every live test MUST run against a freshly-created workspace (do NOT reuse a previously-tested one). A stale --data-dir replica or a leftover docker container carries an old security_fingerprint and corrupts the propagation observation."
  - "`docker ps` succeeds (daemon reachable)"
  - "`curl --version` returns a version string"
  - "`cargo --version` returns a version string (PgPaw built from this repo)"
  - "No container named `pgpaw-ddl-pg` exists (`docker ps -a --filter name=pgpaw-ddl-pg --format '{{.Names}}'` prints nothing)"
  - "TCP port 55433 on 127.0.0.1 is free (`lsof -i :55433` prints nothing)"
  - "TCP port 8081 on 127.0.0.1 is free (`lsof -i :8081` prints nothing)"
  - "The data dir `/tmp/pgpaw-ddl-data` does not yet exist (`test -d /tmp/pgpaw-ddl-data` returns non-zero)"
expected_duration_secs: 540
tags: [replica, ddl, security, rls, propagation, classification, live]
priority: high
created: 2026-06-16
author: senior-qa
---

## Objective

Verify that a table which starts PUBLIC (RLS off, granted to PUBLIC) and is served token-free as a `303` snapshot is, after upstream is switched to RLS + policy + role-grant while PgPaw keeps running, re-classified from public to private within the replica's security resync interval (~60s) so the same no-token query flips to `401` and tenant tokens thereafter see only their own rows.

## Preconditions

- The fixed JWT secret below (`pgpaw-test-secret-please-change`) will be passed to PgPaw via `--jwt-secret`; the tokens in Inputs are pre-signed against it
- The upstream container exposes `5432` on host port `55433` (`-p 55433:5432`)
- PgPaw will bind `127.0.0.1:8081` and replicate the default publication `cache_server_pub`

## Inputs

Fixed HS256 secret PgPaw is launched with:

```text
pgpaw-test-secret-please-change
```

Pre-signed test tokens (HS256, `role=member`, long-lived `exp`), reused verbatim from the jwt-rls-enforcement test. Tenant is carried in `org_id`:

```text
# Tenant A — org_id = 1
TOKEN_A=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoxLCJleHAiOjIwOTY5MDQ2MzF9.KGb25waEy8TTsaWqOzsENgQ8wU0EkyMjrCFiX3NHhDI

# Tenant B — org_id = 2
TOKEN_B=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoyLCJleHAiOjIwOTY5MDQ2MzF9.qxynUpmoxK4Bp4KvBXA0BVIgbLaqR3yBcW0XOsVcfis
```

Schema + initial seed (table starts RLS OFF, granted PUBLIC) — applied one statement per Step in `## Steps`:

```sql
-- docs starts fully public: RLS off, SELECT granted to PUBLIC
CREATE TABLE docs (id int PRIMARY KEY, org_id int, title text);
INSERT INTO docs VALUES
  (101,1,'A-doc-one'), (102,1,'A-doc-two'),
  (201,2,'B-doc-one'), (202,2,'B-doc-two'), (203,2,'B-doc-three');
GRANT SELECT ON docs TO PUBLIC;
CREATE PUBLICATION cache_server_pub FOR ALL TABLES;
```

Security DDL applied LIVE (after launch), one statement per Step:

```sql
CREATE ROLE member LOGIN;
GRANT SELECT ON docs TO member;
ALTER TABLE docs ENABLE ROW LEVEL SECURITY;
ALTER TABLE docs FORCE ROW LEVEL SECURITY;
CREATE POLICY docs_by_org ON docs FOR SELECT TO member
  USING ( org_id = ((select current_setting('request.jwt.claims', true))::json->>'org_id')::int );
```

Tenant partition (ground truth the tester judges row results against):

```text
org_id=1 (Tenant A) owns doc ids: 101, 102
org_id=2 (Tenant B) owns doc ids: 201, 202, 203
```

## Steps

> **Notebook discipline**: Each Step is ONE action. The tester runs it, observes, judges, then moves to the next. No `&&`, `;`, multi-line shells, or for-loops inside a Step's Action. Polling = a Step the tester runs N times.

### Step 1: Provision a fresh logical-replication Postgres

**Action**: `docker run -d --name pgpaw-ddl-pg -e POSTGRES_PASSWORD=postgres -p 55433:5432 postgres:16 -c wal_level=logical`

**Observe**:
- Command exits 0 and prints a 64-hex container id
- `docker ps --filter name=pgpaw-ddl-pg` shows the container as Up

**Awareness**:
- A non-zero exit citing "port is already allocated" means 55433 was not actually free; abort and re-check the precondition.
- Confirm no OTHER postgres container is bound to 55433 that could shadow this one.

**On weirdness**: abort

### Step 2: Confirm wal_level is logical

**Action**: `docker exec pgpaw-ddl-pg psql -U postgres -tAc "SHOW wal_level"`

**Observe**:
- Output is exactly `logical`

**Awareness**:
- If this prints `replica`, the `-c wal_level=logical` flag did not take — logical replication never starts and the whole propagation premise is moot. Treat a wrong value as a hard stop.
- A "the database system is starting up" error means Postgres is not ready yet; wait a few seconds and retry.

**On weirdness**: retry once (for startup race); abort if value is not `logical`

### Step 3: Create the docs table (RLS OFF at birth)

**Action**: `docker exec pgpaw-ddl-pg psql -U postgres -c "CREATE TABLE docs (id int PRIMARY KEY, org_id int, title text)"`

**Observe**:
- Output is `CREATE TABLE`

**Awareness**:
- The table is created with NO row-level security and NO policy on purpose — that is what makes it start PUBLIC. Watch for any NOTICE about an existing relation, which would mean the container is not actually fresh.

**On weirdness**: abort

### Step 4: Seed the cross-tenant rows

**Action**: `docker exec pgpaw-ddl-pg psql -U postgres -c "INSERT INTO docs VALUES (101,1,'A-doc-one'),(102,1,'A-doc-two'),(201,2,'B-doc-one'),(202,2,'B-doc-two'),(203,2,'B-doc-three')"`

**Observe**:
- Output is `INSERT 0 5`

**Awareness**:
- This is the overlapping cross-tenant dataset the isolation assertion hinges on — 2 rows for org 1, 3 rows for org 2. A count other than 5 makes the later disjointness check meaningless.

**On weirdness**: abort

### Step 5: Grant public read on docs (classifies it PUBLIC)

**Action**: `docker exec pgpaw-ddl-pg psql -U postgres -c "GRANT SELECT ON docs TO PUBLIC"`

**Observe**:
- Output is `GRANT`

**Awareness**:
- This grant, combined with RLS being off, is exactly what makes PgPaw classify `docs` as PUBLIC. Without it the baseline `303` in Step 9 will not happen and the public→private flip cannot be demonstrated.

**On weirdness**: abort

### Step 6: Create the publication PgPaw replicates

**Action**: `docker exec pgpaw-ddl-pg psql -U postgres -c "CREATE PUBLICATION cache_server_pub FOR ALL TABLES"`

**Observe**:
- Output is `CREATE PUBLICATION`

**Awareness**:
- The name must be exactly `cache_server_pub` (PgPaw's default `--publication`). FOR ALL TABLES is what lets the policy/role/grant changes later reach the replica's security introspection. A typo means nothing replicates.

**On weirdness**: abort

### Step 7: Launch PgPaw against the temp Postgres (background)

**Action**: `cargo run --release -- serve --pg-host 127.0.0.1 --pg-port 55433 --pg-user postgres --pg-password postgres --pg-database postgres --data-dir /tmp/pgpaw-ddl-data --port 8081 --jwt-secret pgpaw-test-secret-please-change`

**Observe**:
- Process stays running (does not exit); startup logs show it connecting to upstream 55433 and beginning replication
- A fresh `/tmp/pgpaw-ddl-data` directory appears

**Awareness**:
- Run this in the background and capture stdout/stderr to a log the later Steps can tail. The first `cargo run --release` may be a slow build — that is normal, not a failure.
- Watch for "JWT verification is not configured" or a key-parse error — that would mean `--jwt-secret` did not register and every private query later 401s regardless of token, masking the propagation result.
- The background shell wrapper may report completion while the server child keeps running. Verify with `lsof -i :8081` — a present LISTEN line means it is alive; confirm it bound `127.0.0.1:8081`, not a port from a stale env var.

**On weirdness**: abort

### Step 8: Poll until the replica catches up (run N times)

**Action**: `curl -s http://127.0.0.1:8081/healthz`

**Observe**:
- Body is JSON with `"status":"ok"` and a numeric `watermark`
- Re-run until `status` is `ok` with a non-zero watermark; a few `halted`/zero-watermark polls right after launch are expected

**Awareness**:
- If `status` stays `halted`, read the `reason` — a halted replica makes every `/query` return 503, which is NOT the classification behavior under test. Distinguish 503 (replica) from 303/401 (classification).
- Do not run the baseline until at least one `ok` poll is seen, or `docs` may not yet be replicated and the baseline `303` would be unreliable.

**On weirdness**: retry once per poll; abort if still `halted` after several polls

### Step 9: BASELINE — no-token query proves docs starts PUBLIC

**Action**: `curl -s -i -X POST http://127.0.0.1:8081/query -H 'content-type: application/json' -d '{"sql":"select * from docs order by id"}'`

**Observe**:
- Status line is `303 See Other`
- A `Location:` header matches the shape `/q/{hash}/{version}` (hash and version are opaque; only the shape is fixed)

**Awareness**:
- The `303` itself carries `Cache-Control: no-store`; the long-lived `public, max-age=259200` lives on the followed `/q/...` fetch, not the redirect.
- If this is already `401` BEFORE any security DDL, the table was misconfigured (RLS already on, or PUBLIC grant missing) — the whole test is invalid; abort and re-check Steps 3 and 5.

**On weirdness**: abort

### Step 10: Apply security DDL — create the login role

**Action**: `docker exec pgpaw-ddl-pg psql -U postgres -c "CREATE ROLE member LOGIN"`

**Observe**:
- Output is `CREATE ROLE`

**Awareness**:
- `member` is created LOGIN upstream, but the replica re-creates it NOLOGIN NOBYPASSRLS during resync — so the replicated role can never bypass RLS even though the upstream one can log in. A "role already exists" notice means a non-fresh container.

**On weirdness**: abort

### Step 11: Apply security DDL — grant member read on docs

**Action**: `docker exec pgpaw-ddl-pg psql -U postgres -c "GRANT SELECT ON docs TO member"`

**Observe**:
- Output is `GRANT`

**Awareness**:
- Without this grant, after RLS is enabled an authenticated member hits a privilege error that would surface as a `403`, not the scoped `200` the test expects. This grant must replicate alongside the policy.

**On weirdness**: abort

### Step 12: Apply security DDL — enable RLS on docs

**Action**: `docker exec pgpaw-ddl-pg psql -U postgres -c "ALTER TABLE docs ENABLE ROW LEVEL SECURITY"`

**Observe**:
- Output is `ALTER TABLE`

**Awareness**:
- Enabling RLS is the single change that flips `docs` from public to private in PgPaw's classifier — but only AFTER the replica resyncs security and bumps its version. Do not expect an immediate flip in `/query`; that is what the propagation-wait Step measures.

**On weirdness**: abort

### Step 13: Apply security DDL — force RLS on docs

**Action**: `docker exec pgpaw-ddl-pg psql -U postgres -c "ALTER TABLE docs FORCE ROW LEVEL SECURITY"`

**Observe**:
- Output is `ALTER TABLE`

**Awareness**:
- FORCE ensures the policy applies even to a table owner; without it an owning role could bypass the filter. Confirm no error.

**On weirdness**: abort

### Step 14: Apply security DDL — create the tenant-scoping policy

**Action**: `docker exec pgpaw-ddl-pg psql -U postgres -c "CREATE POLICY docs_by_org ON docs FOR SELECT TO member USING ( org_id = ((select current_setting('request.jwt.claims', true))::json->>'org_id')::int )"`

**Observe**:
- Output is `CREATE POLICY`

**Awareness**:
- The policy reads `request.jwt.claims` inline (no helper function) so it can be replicated into the cache. A syntax error here leaves `docs` readable by no member (empty results for every tenant after the flip).

**On weirdness**: abort

### Step 15: PROPAGATION WAIT — poll the no-token query until it flips 303 → 401 (run N times)

**Action**: `curl -s -o /dev/null -w '%{http_code}\n' -X POST http://127.0.0.1:8081/query -H 'content-type: application/json' -d '{"sql":"select * from docs order by id"}'`

**Observe**:
- Re-run this Step repeatedly. It prints `303` immediately after the DDL and must eventually print `401` once the replica resyncs security and re-classifies `docs` as private
- The transition from `303` to `401` is the core propagation assertion of this test — record roughly how long it took

**Awareness**:
- This can take up to the ~60s security resync interval (`role_poll_interval`). A run of `303`s right after the DDL is EXPECTED, not a failure — keep polling.
- A `503` here means the replica halted (e.g., on an incompatible change), which is distinct from a slow flip; read `/healthz` `reason` before concluding propagation failed.
- If it never flips after well over 60s of polling, the resync/`bump_security` path or the policy replication is broken — escalate per Fail Modes.

**On weirdness**: note-and-continue (keep polling); escalate only after clearly exceeding ~60s

### Step 16: Confirm the flip — no-token query is now access-controlled

**Action**: `curl -s -i -X POST http://127.0.0.1:8081/query -H 'content-type: application/json' -d '{"sql":"select * from docs order by id"}'`

**Observe**:
- Status is `401 Unauthorized`
- Error envelope conveys that a bearer token is required (wording may vary; meaning must be "auth required")

**Awareness**:
- It must be `401`, NOT a `200` with zero rows — a zero-row `200` would mean PgPaw ran the now-private query anonymously (fail-open) instead of refusing it.
- This must be the SAME query body that returned `303` in Step 9; only the upstream security state changed, not the SQL.

**On weirdness**: abort

### Step 17: Tenant A token sees only org-1 rows

**Action**: `curl -s -i -X POST http://127.0.0.1:8081/query -H 'content-type: application/json' -H 'authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoxLCJleHAiOjIwOTY5MDQ2MzF9.KGb25waEy8TTsaWqOzsENgQ8wU0EkyMjrCFiX3NHhDI' -d '{"sql":"select * from docs order by id"}'`

**Observe**:
- Status is `200 OK` with header `Cache-Control: private, no-store`
- Body is inline JSON (NOT a `303` redirect) containing ONLY ids 101 and 102; it contains ZERO org-2 rows (no 201/202/203)

**Awareness**:
- This proves the REPLICATED policy enforces per-request claims. Scan the body for any `org_id":2` or B-doc title — its presence is a critical leak even if the count looks right.
- `private, no-store` plus an inline body together prove the access-controlled path was taken, not the cache. A `303` here would mean the flip in Step 16 was illusory.

**On weirdness**: abort

### Step 18: Tenant B token sees only org-2 rows (disjoint from A)

**Action**: `curl -s -i -X POST http://127.0.0.1:8081/query -H 'content-type: application/json' -H 'authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoyLCJleHAiOjIwOTY5MDQ2MzF9.qxynUpmoxK4Bp4KvBXA0BVIgbLaqR3yBcW0XOsVcfis' -d '{"sql":"select * from docs order by id"}'`

**Observe**:
- Status is `200 OK` with `Cache-Control: private, no-store`
- Body contains ONLY ids 201, 202, 203; ZERO org-1 rows (no 101/102)
- The result set is disjoint from Step 17 — no overlap at all

**Awareness**:
- Same role (`member`), different `org_id` claim, different result set — this proves the filter keys on the per-request claim, not a cached or process-wide value. If B sees A's rows (or vice versa), claims are not being applied per request.

**On weirdness**: abort

### Step 19: BIDIRECTIONAL — disable RLS upstream, then poll until the query re-widens 401 → 303 (run N times)

**Action**: `docker exec pgpaw-ddl-pg psql -U postgres -c "ALTER TABLE docs DISABLE ROW LEVEL SECURITY"`

**Observe**:
- Output is `ALTER TABLE`

**Awareness**:
- This is the FIRST half of the bidirectional check — it only changes upstream. The replica will not re-widen until the next security resync. The actual re-widen is observed in Step 20.
- `docs` is still granted to PUBLIC (Step 5 was never revoked), so once RLS is off it should classify PUBLIC again. If the PUBLIC grant had been revoked, it would stay private and this check would be invalid.

**On weirdness**: note-and-continue (this optional check is non-blocking for the core assertion)

### Step 20: BIDIRECTIONAL — poll until the no-token query returns 303 again (run N times)

**Action**: `curl -s -o /dev/null -w '%{http_code}\n' -X POST http://127.0.0.1:8081/query -H 'content-type: application/json' -d '{"sql":"select * from docs order by id"}'`

**Observe**:
- Re-run repeatedly. It prints `401` immediately after Step 19 and must eventually print `303` once the replica resyncs and re-classifies `docs` as public again
- The `401 → 303` transition proves propagation is bidirectional (private→public as well as public→private)

**Awareness**:
- Same ~60s resync latency applies; a run of `401`s right after Step 19 is EXPECTED. Keep polling.
- This Step is optional/nice-to-have. If it does not flip within the resync window, note it as a secondary observation — it does not invalidate the primary public→private result from Steps 15–18.

**On weirdness**: note-and-continue

## Expected Behavior

- BEFORE any security DDL, a no-token query touching `docs` (RLS off, granted PUBLIC) is served as `303 See Other` whose `Location` matches `/q/{hash}/{version}` — the table is genuinely public.
- After upstream applies ENABLE RLS + FORCE RLS + a `member`-scoped policy + the `member` grant while PgPaw keeps running, the SAME no-token query transitions from `303` to `401` WITHOUT any PgPaw restart. The transition completes within roughly the security-resync interval (~60s); a few `303`s during the window are expected, not a failure.
- Once flipped, the no-token query is `401` (auth required), never a zero-row `200`. With Tenant A's token the result is exactly ids 101,102; with Tenant B's token exactly ids 201,202,203; the two sets are disjoint and carry `Cache-Control: private, no-store` inline (never a `303`, never the shared cache) — proving the replicated policy enforces the per-request `request.jwt.claims`.
- Propagation is bidirectional: disabling RLS upstream (with the PUBLIC grant still in place) eventually re-widens the same no-token query back to `303` within the resync window.
- Throughout, `/healthz` reports `status: ok` with a monotonic watermark; the replicated `member` role is NOLOGIN NOBYPASSRLS so Postgres-in-the-replica — not PgPaw — is the row-filtering authority.

Reserve exact-match only for system-composed artifacts: the status codes (`303`, `401`, `200`), the header strings (`Cache-Control: private, no-store`, `Cache-Control: no-store`), and the `/q/{hash}/{version}` Location shape. Row counts/content are judged against the seeded tenant partition (A: 101,102 / B: 201,202,203), not by exact byte match of the JSON; timing is judged against the ~60s window, not a fixed number of polls.

## Fail Modes

- **The no-token query never flips from `303` to `401`** — security resync is not running or the policy/role/grant did not reach the replica → confirm `/healthz` is `ok` (not halted), then check the resync path: the upstream policy/role/grant must be inside publication `cache_server_pub` (it is FOR ALL TABLES), `security_fingerprint` must change, `bump_security` must increment `security_version`, and PgPaw's verdict cache (keyed on `security_version()`) must therefore invalidate. Allow well over 60s before declaring failure.
- **Flips to `401` but every tenant token returns an empty body** — policy replicated but claims not forwarded, or policy syntax issue → verify `docs_by_org` reads `request.jwt.claims` inline (no helper function), and that PgPaw is issuing `SET LOCAL request.jwt.claims` / `SET LOCAL ROLE member` per request.
- **Tenant A sees Tenant B's rows (leak)** — replicated role wrongly has BYPASSRLS, or claims not applied per request → confirm the response carried `private, no-store` (the access-controlled path, not a cached snapshot) and that the replica role is NOLOGIN NOBYPASSRLS.
- **`503` on every `/query`** — replica halted (e.g., an incompatible schema/security change), NOT a classification result → read `/healthz` `reason`; do not score classification steps until `/healthz` is `ok`.
- **Baseline Step 9 is already `401`** — `docs` was not actually public at launch (RLS already on, or PUBLIC grant missing) → the whole test is invalid; re-check Steps 3 and 5 and restart against a fresh workspace.

## Cleanup

### Cleanup 1: Stop and remove the Postgres container

**Action**: `docker rm -f pgpaw-ddl-pg`

**Observe**:
- Output prints the container name; `docker ps -a --filter name=pgpaw-ddl-pg` is then empty

**Awareness**:
- A leftover container holds port 55433 and breaks the next fresh run.

**On weirdness**: note-and-continue

### Cleanup 2: Stop the PgPaw server process by its unique data-dir

**Action**: `pkill -f '/tmp/pgpaw-ddl-data'`

**Observe**:
- The background PgPaw process exits; port 8081 is freed (`lsof -i :8081` prints nothing)

**Awareness**:
- The process runs as `pgpaw serve`, so a pattern like `cache-server.*serve` does NOT match — killing by the unique `--data-dir` path `/tmp/pgpaw-ddl-data` is the robust pattern.
- If the socket lingers in `CLOSE_WAIT`, use `kill -9 <PID>` to force-terminate and free the port.

**On weirdness**: note-and-continue

### Cleanup 3: Remove the replica data directory

**Action**: `rm -rf /tmp/pgpaw-ddl-data`

**Observe**:
- `/tmp/pgpaw-ddl-data` no longer exists (`test -d /tmp/pgpaw-ddl-data` returns non-zero)

**Awareness**:
- Reusing this dir on a later run reuses a stale replica with an old security_fingerprint and violates the FRESH WORKSPACE contract.

**On weirdness**: note-and-continue
