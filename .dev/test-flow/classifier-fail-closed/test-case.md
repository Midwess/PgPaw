---
id: classifier-fail-closed
title: The public/private classifier fails closed on every ambiguous or edge case and never serves access-controlled data as a cacheable public snapshot
tier: live
component: classify
target: pgpaw (cache-server binary)
prerequisites:
  - "FRESH WORKSPACE REQUIRED — every live test MUST run against a freshly-created workspace (do NOT reuse a previously-tested one). A stale --data-dir replica or a leftover docker container contaminates classification observations."
  - "`docker --version` returns a version string"
  - "`curl --version` returns a version string"
  - "`cargo --version` returns a version string (PgPaw built from this repo, or `pgpaw` on PATH)"
  - "`docker ps` succeeds (daemon reachable)"
  - "No container named `pgpaw-cls-pg` already exists (`docker ps -a --filter name=pgpaw-cls-pg --format '{{.Names}}'` prints nothing)"
  - "TCP port 55434 on 127.0.0.1 is free (`lsof -i :55434` prints nothing)"
  - "TCP port 8082 on 127.0.0.1 is free (`lsof -i :8082` prints nothing)"
  - "The data dir `/tmp/pgpaw-cls-data` does not yet exist (`test -d /tmp/pgpaw-cls-data` returns non-zero)"
expected_duration_secs: 480
tags: [classify, fail-closed, public-private, rls, access-control, cache, live]
priority: high
created: 2026-06-16
author: senior-qa
---

## Objective

Verify that PgPaw's public/private classifier treats a query as public ONLY IF every referenced table has RLS off AND SELECT granted to PUBLIC, and fails closed (private / 4xx, never a cacheable public `303`) for every other shape: RLS-on-no-policy, mixed public+private joins, revoked PUBLIC grant, a relname colliding across schemas where any copy is private, and unknown tables.

## Preconditions

- `docker ps` succeeds (daemon reachable)
- The fixed JWT secret below (`pgpaw-test-secret-please-change`) will be passed to PgPaw via `--jwt-secret`; the token in Inputs is pre-signed against it
- No container named `pgpaw-cls-pg` already exists (`docker ps -a --filter name=pgpaw-cls-pg --format '{{.Names}}'` prints nothing)
- The data dir `/tmp/pgpaw-cls-data` does not yet exist (`test -d /tmp/pgpaw-cls-data` returns non-zero)

## Inputs

Fixed HS256 secret PgPaw is launched with:

```text
pgpaw-test-secret-please-change
```

Pre-signed test token (HS256, `role=member`, long-lived `exp`, `org_id=1`) — reused from the jwt-rls-enforcement exemplar:

```text
# TOKEN_A — role=member, org_id=1
TOKEN_A=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoxLCJleHAiOjIwOTY5MDQ2MzF9.KGb25waEy8TTsaWqOzsENgQ8wU0EkyMjrCFiX3NHhDI
```

Schema + seed (DDL applied one statement per Step in `## Steps`):

```sql
-- pub_t: the genuinely-public control (RLS off, SELECT granted to PUBLIC)
CREATE TABLE pub_t (id int PRIMARY KEY, v text);
INSERT INTO pub_t VALUES (1,'p-one'), (2,'p-two');
GRANT SELECT ON pub_t TO PUBLIC;

-- pub_t2: a SECOND genuinely-public table, used only by the revoke case so it
-- does not couple to the duplicate-relname check on pub_t
CREATE TABLE pub_t2 (id int PRIMARY KEY, v text);
INSERT INTO pub_t2 VALUES (1,'q-one'), (2,'q-two');
GRANT SELECT ON pub_t2 TO PUBLIC;

-- rls_nopolicy: RLS ENABLED but NO policy created => deny-all for non-owner
CREATE TABLE rls_nopolicy (id int PRIMARY KEY, v text);
INSERT INTO rls_nopolicy VALUES (1,'secret-a'), (2,'secret-b');
ALTER TABLE rls_nopolicy ENABLE ROW LEVEL SECURITY;
GRANT SELECT ON rls_nopolicy TO member;

-- secret_t: a normal private table (RLS on + policy + grant to member)
CREATE TABLE secret_t (id int PRIMARY KEY, org_id int, v text);
INSERT INTO secret_t VALUES (1,1,'s-one'), (2,1,'s-two');
ALTER TABLE secret_t ENABLE ROW LEVEL SECURITY;
GRANT SELECT ON secret_t TO member;
CREATE POLICY secret_by_org ON secret_t FOR SELECT TO member
  USING ( org_id = ((select current_setting('request.jwt.claims', true))::json->>'org_id')::int );

-- s2.pub_t: same RELNAME as public.pub_t but RLS ON, in a different schema,
-- to probe duplicate-relname fail-closed
CREATE SCHEMA s2;
CREATE TABLE s2.pub_t (id int PRIMARY KEY, v text);
INSERT INTO s2.pub_t VALUES (1,'s2-one');
ALTER TABLE s2.pub_t ENABLE ROW LEVEL SECURITY;

-- login role the JWT role claim maps to
CREATE ROLE member LOGIN;

-- publication PgPaw replicates
CREATE PUBLICATION cache_server_pub FOR ALL TABLES;
```

Classification ground truth the tester judges against:

```text
public.pub_t   -> PUBLIC   (RLS off AND granted PUBLIC)        => 303 with no token
public.pub_t2  -> PUBLIC   (until revoked in Step 23)          => 303, then 401 after revoke
rls_nopolicy   -> PRIVATE  (RLS on, no PUBLIC grant)           => 401 no token; 200 zero rows with token
secret_t       -> PRIVATE  (RLS on)                            => taints any join it appears in
s2.pub_t       -> PRIVATE  (RLS on; relname collides w/ pub_t) => 401, must NOT corrupt public.pub_t
does_not_exist -> not replicated                               => 4xx Rejected at classify
```

## Steps

> **Notebook discipline**: Each Step is ONE action. The tester runs it, observes, judges, then moves to the next. No `&&`, `;`, multi-line shells, or for-loops inside a Step's Action. Polling = a Step the tester runs N times.

### Step 1: Provision a fresh logical-replication Postgres

**Action**: `docker run -d --name pgpaw-cls-pg -e POSTGRES_PASSWORD=postgres -p 55434:5432 postgres:16 -c wal_level=logical`

**Observe**:
- Command exits 0 and prints a 64-hex container id
- `docker ps --filter name=pgpaw-cls-pg` shows the container as Up

**Awareness**:
- A non-zero exit citing "port is already allocated" means port 55434 was not actually free; abort and re-check the precondition. A slow first-run image pull is normal, not a failure.
- Confirm no OTHER postgres container is bound to 55434 that could shadow this one.

**On weirdness**: abort

### Step 2: Confirm wal_level is logical

**Action**: `docker exec pgpaw-cls-pg psql -U postgres -tAc "SHOW wal_level"`

**Observe**:
- Output is exactly `logical`

**Awareness**:
- If this prints `replica` the `-c wal_level=logical` flag did not take — replication will silently never start and every table will look unreplicated (400). A "system is starting up" error means Postgres is not ready yet; wait a few seconds and retry.

**On weirdness**: retry once (startup race); abort if value is not `logical`

### Step 3: Create the public control table pub_t

**Action**: `docker exec pgpaw-cls-pg psql -U postgres -c "CREATE TABLE pub_t (id int PRIMARY KEY, v text)"`

**Observe**:
- Output is `CREATE TABLE`

**Awareness**:
- A NOTICE about an existing relation means the container is not actually fresh; abort.

**On weirdness**: abort

### Step 4: Seed pub_t rows

**Action**: `docker exec pgpaw-cls-pg psql -U postgres -c "INSERT INTO pub_t VALUES (1,'p-one'),(2,'p-two')"`

**Observe**:
- Output is `INSERT 0 2`

**Awareness**:
- A count other than 2 means a partial seed; the public-control assertion becomes unreliable.

**On weirdness**: abort

### Step 5: Grant PUBLIC select on pub_t

**Action**: `docker exec pgpaw-cls-pg psql -U postgres -c "GRANT SELECT ON pub_t TO PUBLIC"`

**Observe**:
- Output is `GRANT`

**Awareness**:
- This grant plus RLS-off is the ONLY thing that makes pub_t classify public. If it is missed, the public-control Step would wrongly 401 and the whole fail-closed test loses its positive control.

**On weirdness**: abort

### Step 6: Create the second public table pub_t2

**Action**: `docker exec pgpaw-cls-pg psql -U postgres -c "CREATE TABLE pub_t2 (id int PRIMARY KEY, v text)"`

**Observe**:
- Output is `CREATE TABLE`

**Awareness**:
- pub_t2 exists solely to carry the revoke case so it does not couple to pub_t's duplicate-relname check. Confirm no error.

**On weirdness**: abort

### Step 7: Seed pub_t2 rows

**Action**: `docker exec pgpaw-cls-pg psql -U postgres -c "INSERT INTO pub_t2 VALUES (1,'q-one'),(2,'q-two')"`

**Observe**:
- Output is `INSERT 0 2`

**Awareness**:
- A count other than 2 means a partial seed.

**On weirdness**: abort

### Step 8: Grant PUBLIC select on pub_t2

**Action**: `docker exec pgpaw-cls-pg psql -U postgres -c "GRANT SELECT ON pub_t2 TO PUBLIC"`

**Observe**:
- Output is `GRANT`

**Awareness**:
- pub_t2 must START public so the revoke case can demonstrate a public->private flip. If this grant is missed, pub_t2 is private from the start and the revoke Step proves nothing.

**On weirdness**: abort

### Step 9: Create rls_nopolicy

**Action**: `docker exec pgpaw-cls-pg psql -U postgres -c "CREATE TABLE rls_nopolicy (id int PRIMARY KEY, v text)"`

**Observe**:
- Output is `CREATE TABLE`

**Awareness**:
- Confirm no error; this is the deny-all probe.

**On weirdness**: abort

### Step 10: Seed rls_nopolicy rows

**Action**: `docker exec pgpaw-cls-pg psql -U postgres -c "INSERT INTO rls_nopolicy VALUES (1,'secret-a'),(2,'secret-b')"`

**Observe**:
- Output is `INSERT 0 2`

**Awareness**:
- These rows must be physically present so that an empty result later proves RLS deny-all, not an empty table. A count other than 2 makes the "zero rows is a deny, not an empty table" judgment ambiguous.

**On weirdness**: abort

### Step 11: Enable RLS on rls_nopolicy (no policy)

**Action**: `docker exec pgpaw-cls-pg psql -U postgres -c "ALTER TABLE rls_nopolicy ENABLE ROW LEVEL SECURITY"`

**Observe**:
- Output is `ALTER TABLE`

**Awareness**:
- Do NOT create any policy on this table — the whole point is RLS-on-no-policy = deny-all. RLS-on is also exactly what makes the classifier mark it private (first clause of the predicate).

**On weirdness**: abort

### Step 12: Grant member select on rls_nopolicy

**Action**: `docker exec pgpaw-cls-pg psql -U postgres -c "GRANT SELECT ON rls_nopolicy TO member"`

**Observe**:
- Output is `GRANT`

**Awareness**:
- Granted to member only, NOT to PUBLIC — so it is private. Without any grant the token query would 403 (privilege error) instead of the empty `200` we want to prove deny-all.

**On weirdness**: abort

### Step 13: Create secret_t

**Action**: `docker exec pgpaw-cls-pg psql -U postgres -c "CREATE TABLE secret_t (id int PRIMARY KEY, org_id int, v text)"`

**Observe**:
- Output is `CREATE TABLE`

**Awareness**:
- Confirm no error; secret_t is the taint source for the mixed-join case.

**On weirdness**: abort

### Step 14: Seed secret_t rows

**Action**: `docker exec pgpaw-cls-pg psql -U postgres -c "INSERT INTO secret_t VALUES (1,1,'s-one'),(2,1,'s-two')"`

**Observe**:
- Output is `INSERT 0 2`

**Awareness**:
- ids 1 and 2 overlap pub_t's ids on purpose so the mixed join has matching rows; a count other than 2 weakens the join probe.

**On weirdness**: abort

### Step 15: Enable RLS on secret_t

**Action**: `docker exec pgpaw-cls-pg psql -U postgres -c "ALTER TABLE secret_t ENABLE ROW LEVEL SECURITY"`

**Observe**:
- Output is `ALTER TABLE`

**Awareness**:
- This flips secret_t to private; the mixed-join case depends on it.

**On weirdness**: abort

### Step 16: Grant member select on secret_t

**Action**: `docker exec pgpaw-cls-pg psql -U postgres -c "GRANT SELECT ON secret_t TO member"`

**Observe**:
- Output is `GRANT`

**Awareness**:
- Granted to member, NOT PUBLIC, so secret_t stays private.

**On weirdness**: abort

### Step 17: Create the tenant-scoping policy on secret_t

**Action**: `docker exec pgpaw-cls-pg psql -U postgres -c "CREATE POLICY secret_by_org ON secret_t FOR SELECT TO member USING ( org_id = ((select current_setting('request.jwt.claims', true))::json->>'org_id')::int )"`

**Observe**:
- Output is `CREATE POLICY`

**Awareness**:
- The policy reads `request.jwt.claims` inline (no helper function). It only matters for secret_t row content; the classifier decision keys on RLS-on, not on the policy body.

**On weirdness**: abort

### Step 18: Create schema s2

**Action**: `docker exec pgpaw-cls-pg psql -U postgres -c "CREATE SCHEMA s2"`

**Observe**:
- Output is `CREATE SCHEMA`

**Awareness**:
- s2 hosts the colliding relname; a failure here means the duplicate-relname case cannot be exercised.

**On weirdness**: abort

### Step 19: Create s2.pub_t (the colliding RLS-on relname)

**Action**: `docker exec pgpaw-cls-pg psql -U postgres -c "CREATE TABLE s2.pub_t (id int PRIMARY KEY, v text)"`

**Observe**:
- Output is `CREATE TABLE`

**Awareness**:
- The RELNAME must be exactly `pub_t` (same bare name as public.pub_t, different schema) — the classifier keys on bare relname, so this collision is the entire point. A typo in the name silently disarms the cross-schema probe.

**On weirdness**: abort

### Step 20: Seed and lock s2.pub_t with RLS on

**Action**: `docker exec pgpaw-cls-pg psql -U postgres -c "INSERT INTO s2.pub_t VALUES (1,'s2-one')"`

**Observe**:
- Output is `INSERT 0 1`

**Awareness**:
- One row is enough; this row must NEVER appear in any public snapshot. It exists so a leak would be visible if the collision were resolved public.

**On weirdness**: abort

### Step 21: Enable RLS on s2.pub_t

**Action**: `docker exec pgpaw-cls-pg psql -U postgres -c "ALTER TABLE s2.pub_t ENABLE ROW LEVEL SECURITY"`

**Observe**:
- Output is `ALTER TABLE`

**Awareness**:
- This is what makes the s2 copy private. With RLS off here, the collision would be public+public and prove nothing — so confirm RLS is actually ON (`\d+ s2.pub_t` shows "Row security: enabled" if in doubt).

**On weirdness**: abort

### Step 22: Create the login role member

**Action**: `docker exec pgpaw-cls-pg psql -U postgres -c "CREATE ROLE member LOGIN"`

**Observe**:
- Output is `CREATE ROLE`

**Awareness**:
- `member` must NOT be a superuser and must NOT have BYPASSRLS — either would let the deny-all and tenant filters be bypassed, masking a real classifier behavior with a privilege artifact. A "role already exists" notice means a non-fresh container.

**On weirdness**: abort

### Step 23: Create the publication PgPaw replicates

**Action**: `docker exec pgpaw-cls-pg psql -U postgres -c "CREATE PUBLICATION cache_server_pub FOR ALL TABLES"`

**Observe**:
- Output is `CREATE PUBLICATION`

**Awareness**:
- The name must be exactly `cache_server_pub` (PgPaw's default `--publication`). A typo means nothing replicates and every query 400s as "not replicated", which is NOT the fail-closed behavior under test.

**On weirdness**: abort

### Step 24: Launch PgPaw against the temp Postgres (background)

**Action**: `cargo run --release -- serve --pg-host 127.0.0.1 --pg-port 55434 --pg-user postgres --pg-password postgres --pg-database postgres --data-dir /tmp/pgpaw-cls-data --port 8082 --jwt-secret pgpaw-test-secret-please-change`

**Observe**:
- Process stays running (does not exit); startup logs show it connecting upstream and beginning replication
- A fresh `/tmp/pgpaw-cls-data` directory appears

**Awareness**:
- Run in the background and capture stdout/stderr to a log the later Steps can tail. Watch for "JWT verification is not configured" — that would make the token cases 401 regardless of validity.
- Confirm the bind line shows `127.0.0.1:8082`, not a port from a stale env var. After launch, verify via `lsof -i :8082` (a LISTEN line means it is running even if the background shell wrapper reported completion).

**On weirdness**: abort

### Step 25: Poll until the replica catches up (run N times)

**Action**: `curl -s http://127.0.0.1:8082/healthz`

**Observe**:
- Body is JSON with `"status":"ok"` and a numeric `watermark`
- Re-run until `status` is `ok` with a non-zero watermark; a few `halted`/zero-watermark polls right after launch are expected

**Awareness**:
- If `status` stays `halted`, read `reason` — a halted replica makes every `/query` return 503, which is NOT a classification decision. Distinguish 503 (replica) from 303/401/400 (classifier).
- Do not run any classification assertion until at least one `ok` poll is seen, or s2.pub_t / rls_nopolicy may not be replicated yet and would 400 spuriously.

**On weirdness**: retry once per poll; abort if still `halted` after several polls

### Step 26: Control — public query with NO token

**Action**: `curl -s -i -X POST http://127.0.0.1:8082/query -H 'content-type: application/json' -d '{"sql":"select * from pub_t order by id"}'`

**Observe**:
- Status line is `303 See Other`
- A `Location:` header matches the shape `/q/{hash}/{version}`

**Awareness**:
- This is the positive control: it proves the classifier CAN see public when a table is truly public (RLS off AND PUBLIC grant). If this 401s, the classifier is broken in the public direction and the rest of the fail-closed assertions cannot be interpreted — fix the grant/RLS state first.
- The 303 itself carries `Cache-Control: no-store` (the long-lived public cache header lives on the followed `/q/...` fetch). Note the Location to optionally follow it.

**On weirdness**: abort

### Step 27: Fail-closed — RLS-on-no-policy with NO token

**Action**: `curl -s -i -X POST http://127.0.0.1:8082/query -H 'content-type: application/json' -d '{"sql":"select * from rls_nopolicy order by id"}'`

**Observe**:
- Status is `401 Unauthorized`
- No `Location:` header and no rows in the body

**Awareness**:
- It must be `401`, NOT a `303` and NOT a zero-row `200`. A `303`/`public,max-age` here is a fail-OPEN leak (RLS-on table served as a cacheable snapshot). A zero-row `200` would mean PgPaw ran a private query anonymously.

**On weirdness**: abort (record which response was seen)

### Step 28: Deny-all — RLS-on-no-policy WITH TOKEN_A

**Action**: `curl -s -i -X POST http://127.0.0.1:8082/query -H 'content-type: application/json' -H 'authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJyb2xlIjoibWVtYmVyIiwib3JnX2lkIjoxLCJleHAiOjIwOTY5MDQ2MzF9.KGb25waEy8TTsaWqOzsENgQ8wU0EkyMjrCFiX3NHhDI' -d '{"sql":"select * from rls_nopolicy order by id"}'`

**Observe**:
- Status is `200 OK` with header `Cache-Control: private, no-store`
- Body is an EMPTY JSON array (`[]`) — zero rows — and is inline JSON, not a redirect

**Awareness**:
- The seeded rows `secret-a`/`secret-b` must NOT appear. RLS-on-no-policy is deny-all for a non-owner, so an empty private result is correct; any row content here is a leak. Confirm it is `private, no-store` (access-controlled path) and never `303`.
- If this returns `403` instead of an empty `200`, the member grant (Step 12) was missed — that is a privilege artifact, not a classifier failure.

**On weirdness**: abort if any row content appears; note-and-continue on a 403 (diagnose grant)

### Step 29: Mixed query — public+private join with NO token

**Action**: `curl -s -i -X POST http://127.0.0.1:8082/query -H 'content-type: application/json' -d '{"sql":"select p.id from pub_t p join secret_t s on s.id = p.id"}'`

**Observe**:
- Status is `401 Unauthorized`
- No `Location:` header and no rows in the body

**Awareness**:
- A single private table (secret_t) must taint the WHOLE statement to private even though pub_t is public. A `303` here would mean the join was served as a public snapshot — a fail-open leak. The presence of the public pub_t must not pull the verdict back to public.

**On weirdness**: abort (record the status)

### Step 30: Duplicate relname — s2.pub_t (RLS-on copy) with NO token

**Action**: `curl -s -i -X POST http://127.0.0.1:8082/query -H 'content-type: application/json' -d '{"sql":"select * from s2.pub_t"}'`

**Observe**:
- Status is `401 Unauthorized`
- No `Location:` header, no `s2-one` row in the body

**Awareness**:
- The classifier keys on the BARE relname `pub_t`; because the s2 copy has RLS on, the relname must resolve private (fail closed). A `303` here is the cross-schema leak this test exists to catch — it would mean the public copy's verdict was applied to the private copy.

**On weirdness**: abort (record which status; a 303 is critical)

### Step 31: Collision did NOT corrupt the public copy — re-run pub_t with NO token

**Action**: `curl -s -i -X POST http://127.0.0.1:8082/query -H 'content-type: application/json' -d '{"sql":"select * from pub_t order by id"}'`

**Observe**:
- Status is still `303 See Other` with a `/q/{hash}/{version}` Location, identical in shape to Step 26

**Awareness**:
- This runs BEFORE the revoke (Step 33) so public.pub_t is still genuinely public. It proves the collision did not poison the public verdict in the opposite direction (the public table must stay public; only the private s2 copy is private). If this flipped to 401, the collision corrupted the public verdict — note it.
- The revoke case deliberately uses pub_t2, NOT pub_t, so this re-run stays valid regardless of revoke ordering.

**On weirdness**: note-and-continue (record if it is no longer 303)

### Step 32: Unknown table with NO token

**Action**: `curl -s -i -X POST http://127.0.0.1:8082/query -H 'content-type: application/json' -d '{"sql":"select * from does_not_exist"}'`

**Observe**:
- Status is a `400`-class error (Bad Request); the JSON envelope conveys a Rejected/Parse meaning ("not replicated" / not available in this cache)
- Status is NOT `500` and the request does NOT hang

**Awareness**:
- An unknown table is rejected at the classify stage (not replicated) before any security lookup, so the meaning is "rejected", not "unauthorized". A `500` would mean an unhandled error path; a hang would mean the request never returned — both are failures distinct from the intended 4xx fail-closed.

**On weirdness**: abort on 500/hang; note-and-continue if the 4xx code differs but is still a 4xx envelope

### Step 33: Revoke PUBLIC select on pub_t2 (upstream)

**Action**: `docker exec pgpaw-cls-pg psql -U postgres -c "REVOKE SELECT ON pub_t2 FROM PUBLIC"`

**Observe**:
- Output is `REVOKE`

**Awareness**:
- This changes only pub_t2's grant; RLS stays OFF on pub_t2. The classifier's SECOND clause (`not has_table_privilege('public', oid, 'SELECT')`) is what should now flip it to private. The grant change must propagate to the replica's catalog before PgPaw sees it — that propagation is what the next poll Step waits for.

**On weirdness**: abort

### Step 34: Poll until the revoke propagates, then re-query pub_t2 with NO token (run N times)

**Action**: `curl -s -i -X POST http://127.0.0.1:8082/query -H 'content-type: application/json' -d '{"sql":"select * from pub_t2 order by id"}'`

**Observe**:
- Initially may still be `303` (old grant still cached/unpropagated); re-run the same Step until it becomes `401 Unauthorized`
- Once flipped: `401` with no `Location:` and no rows

**Awareness**:
- Propagation can take up to ~60s; an initial `303` is propagation latency, NOT a classification bug. Treat the final, settled response after several polls as the verdict. If it NEVER flips after well past the poll window, re-read `/healthz` (watermark advancing?) to distinguish a stalled replica from a real fail-open.
- Distinguish this from Step 26/31: pub_t (still granted) must remain `303` throughout; only pub_t2 flips. If pub_t also flipped, something other than the revoke is at play.

**On weirdness**: retry per poll up to the window; if still `303` long after ~60s and watermark is advancing, note-and-continue and flag as a suspected fail-open

## Expected Behavior

- Only the truly-public table (RLS off AND granted to PUBLIC) is ever served without a token as a `303 See Other` with a `/q/{hash}/{version}` Location. pub_t in Step 26 (and Step 31) is the sole legitimate `303`.
- Every non-public shape is `401` without a token, never a `303`/`public,max-age` snapshot: RLS-on-no-policy (Step 27), a public+private mixed join (Step 29), the RLS-on colliding relname (Step 30), and the revoked-grant table after propagation (Step 34).
- RLS-on-no-policy is deny-all, not deny-classification: with a valid token it returns `200` `Cache-Control: private, no-store` with an EMPTY array (Step 28) — never a leak, never a `303`.
- Mixing any private table makes the whole query private; the presence of a public table in the join does not pull the verdict back to public.
- A relname colliding across schemas where any copy is private resolves to private (fail closed); the collision must NOT corrupt the genuinely-public copy's verdict (Step 31 stays `303`).
- Revoking PUBLIC select flips a previously-public, RLS-off table to private after propagation, exercising the second clause of the public predicate; sibling public tables (pub_t) are unaffected.
- An unknown/unreplicated table yields a `400`-class error envelope (Rejected/Parse), never a `500` and never a hang.

Reserve exact-match only for system-composed artifacts: the status codes (`303`, `401`, `400`, `200`), the header strings (`Cache-Control: private, no-store`, `Cache-Control: no-store`), and the `/q/{hash}/{version}` Location shape. Row content and envelope wording are judged behaviorally against the classification ground truth in Inputs, not by exact byte match.

## Fail Modes

- **Any edge case returns `303` / `public, max-age`** — fail-OPEN classification leak (critical) → record which Step (27/29/30/34). Confirm the table's RLS/grant state in the container (`\d+ <table>` for "Row security", `\dp <table>` for PUBLIC SELECT) and that PgPaw's security version advanced (the `is_private` cache may be stale only within one security version).
- **rls_nopolicy returns rows for a non-owner (Step 28 leaks)** — the role has BYPASSRLS/superuser, or RLS was not actually enabled → check `\du member` (no Superuser/Bypass RLS) and `\d+ rls_nopolicy` (Row security: enabled). This is a role/policy misconfig, not a classifier bug.
- **rls_nopolicy returns `403` with a token (Step 28)** — the member grant (Step 12) was missed → re-run `\dp rls_nopolicy`; the table is the problem, not the classifier.
- **Revoke (Step 34) never flips to `401`** — propagation latency vs real fail-open → re-read `/healthz` and confirm watermark is advancing; wait the full ~60s poll window. Only after the catalog change is applied AND the security version bumped does the flip become observable. A never-advancing watermark is a replication failure, not a classifier failure.
- **s2.pub_t case (Step 30) returns `303`** — the duplicate-relname fold did not fail closed → confirm s2.pub_t actually has RLS on and that both copies share the bare relname `pub_t`; a `303` here is the cross-schema leak the test targets.
- **Unknown table (Step 32) returns `500` or hangs** — an unhandled error path or a stuck request → check PgPaw logs for a panic/timeout; this is distinct from the intended 4xx Rejected envelope.
- **All queries `503`** — replica halted or never caught up → re-read `/healthz` `reason`; replication failure, not classification. Do not score classification Steps until `/healthz` is `ok`.

## Cleanup

### Cleanup 1: Stop and remove the Postgres container

**Action**: `docker rm -f pgpaw-cls-pg`

**Observe**:
- Output prints the container name; `docker ps -a --filter name=pgpaw-cls-pg` is then empty

**Awareness**:
- A leftover container holds port 55434 and would break the next fresh run.

**On weirdness**: note-and-continue

### Cleanup 2: Stop the PgPaw server process

**Action**: `pkill -f '/tmp/pgpaw-cls-data'`

**Observe**:
- The background PgPaw process exits; port 8082 is freed (`lsof -i :8082` prints nothing)

**Awareness**:
- Kill by the unique data-dir path, not by binary name — `cache-server.*serve` does NOT match this `pgpaw serve` process, and a name-based kill could hit an unrelated pgpaw. If the socket lingers in `CLOSE_WAIT`, force with `kill -9 <PID>`.

**On weirdness**: note-and-continue

### Cleanup 3: Remove the replica data directory

**Action**: `rm -rf /tmp/pgpaw-cls-data`

**Observe**:
- `/tmp/pgpaw-cls-data` no longer exists (`test -d /tmp/pgpaw-cls-data` returns non-zero)

**Awareness**:
- Reusing this dir on a later run reuses a stale replica and violates the FRESH WORKSPACE contract.

**On weirdness**: note-and-continue
