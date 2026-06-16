---
id: query-redirect
title: The public /query path is a correct content-addressed redirect cache — 303 to /q/{hash}/{version}, cacheable 200 snapshot with ETag, idempotent hash, version bump on upstream change, 404 on unknown cursor
tier: live
component: cache
target: pgpaw (cache-server binary)
prerequisites:
  - "FRESH WORKSPACE REQUIRED — every live test MUST run against a freshly-created workspace (do NOT reuse a previously-tested one). A stale --data-dir replica or a leftover docker container contaminates version/watermark and snapshot observations."
  - "`docker --version` returns a version string"
  - "`curl --version` returns a version string"
  - "`cargo --version` returns a version string (PgPaw built from this repo, or `pgpaw` on PATH)"
  - "TCP port 55437 on 127.0.0.1 is free (`lsof -i :55437` prints nothing)"
  - "TCP port 8085 on 127.0.0.1 is free (`lsof -i :8085` prints nothing)"
  - "No container named `pgpaw-redir-pg` already exists (`docker ps -a --filter name=pgpaw-redir-pg --format '{{.Names}}'` prints nothing)"
  - "The data dir `/tmp/pgpaw-redir-data` does not yet exist (`test -d /tmp/pgpaw-redir-data` returns non-zero)"
expected_duration_secs: 360
tags: [cache, redirect, content-addressed, etag, versioning, cdn, live]
priority: high
created: 2026-06-16
author: senior-qa
---

## Objective

Verify that a public `POST /query` returns `303 See Other` with a `Location: /q/{hash}/{version}` whose followed snapshot is a cacheable `200` (`Cache-Control: public, max-age=259200` + `ETag`) carrying the rows, that identical SQL is idempotent (stable hash, stable ETag), that an upstream data change bumps the version into a new immutable snapshot URL, and that an unknown cursor is `404 NotFound` rather than a 500 or hang.

## Preconditions

- `docker ps` succeeds (daemon reachable)
- The publication PgPaw replicates will be named exactly `cache_server_pub` (PgPaw's default `--publication`)
- The `items` table is PUBLIC (granted to PUBLIC, RLS off) so the query path is token-free — no JWT is exercised here
- The fixed JWT secret `pgpaw-test-secret-please-change` is passed only to satisfy startup config; no token is sent on any request in this test

## Inputs

Schema + seed (PUBLIC table; applied one statement per Step in `## Steps`):

```sql
CREATE TABLE items (id int PRIMARY KEY, name text);
INSERT INTO items VALUES (1,'alpha'),(2,'beta');
GRANT SELECT ON items TO PUBLIC;
CREATE PUBLICATION cache_server_pub FOR ALL TABLES;
```

The single SQL statement the redirect/idempotency Steps reuse verbatim:

```text
select id, name from items order by id
```

The upstream change applied in the version-bump Step:

```sql
INSERT INTO items VALUES (3,'gamma');
```

Ground truth the tester judges snapshot bodies against:

```text
Pre-insert snapshot body : rows for id 1 (alpha) and id 2 (beta) only
Post-insert snapshot body: rows for id 1 (alpha), id 2 (beta), id 3 (gamma)
```

## Steps

> **Notebook discipline**: Each Step is ONE action. The tester runs it, observes, judges, then moves to the next. No `&&`, `;`, multi-line shells, or for-loops inside a Step's Action. Polling = a Step the tester runs N times.

### Step 1: Provision a fresh logical-replication Postgres

**Action**: `docker run -d --name pgpaw-redir-pg -e POSTGRES_PASSWORD=postgres -p 55437:5432 postgres:16 -c wal_level=logical`

**Observe**:
- Command exits 0 and prints a 64-hex container id
- `docker ps --filter name=pgpaw-redir-pg` shows the container as Up

**Awareness**:
- This pulls `postgres:16` on first run — a slow pull is normal, not a failure. A non-zero exit citing "port is already allocated" means port 55437 was not actually free; abort and re-check the precondition.
- Confirm no OTHER postgres container is bound to 55437 that could shadow this one.

**On weirdness**: abort

### Step 2: Confirm wal_level is logical

**Action**: `docker exec pgpaw-redir-pg psql -U postgres -tAc "SHOW wal_level"`

**Observe**:
- Output is exactly `logical`

**Awareness**:
- If this prints `replica` the `-c wal_level=logical` flag did not take — logical replication never starts, the watermark never advances, and every version-bump assertion later becomes meaningless. Treat a wrong value as a hard stop.
- A "FATAL: the database system is starting up" error means Postgres is not ready yet; wait a few seconds and retry.

**On weirdness**: retry once (for startup race); abort if value is not `logical`

### Step 3: Create the public items table

**Action**: `docker exec pgpaw-redir-pg psql -U postgres -c "CREATE TABLE items (id int PRIMARY KEY, name text)"`

**Observe**:
- Output is `CREATE TABLE`

**Awareness**:
- Watch for any NOTICE about an existing relation — that would mean the container is not actually fresh.
- A PRIMARY KEY on `id` is required: the version index anchors on the table's replica identity, and a missing PK can change how writes bump the version.

**On weirdness**: abort

### Step 4: Seed the two initial rows

**Action**: `docker exec pgpaw-redir-pg psql -U postgres -c "INSERT INTO items VALUES (1,'alpha'),(2,'beta')"`

**Observe**:
- Output is `INSERT 0 2`

**Awareness**:
- A row count other than 2 means a partial seed; the pre-insert snapshot body (alpha,beta) the whole idempotency check rests on would be wrong.

**On weirdness**: abort

### Step 5: Grant public read on items

**Action**: `docker exec pgpaw-redir-pg psql -U postgres -c "GRANT SELECT ON items TO PUBLIC"`

**Observe**:
- Output is `GRANT`

**Awareness**:
- This is what makes the query classify PUBLIC (no token, cacheable). If it is missed, the no-token query will wrongly demand a token (401) and never produce a 303.

**On weirdness**: abort

### Step 6: Create the publication PgPaw replicates

**Action**: `docker exec pgpaw-redir-pg psql -U postgres -c "CREATE PUBLICATION cache_server_pub FOR ALL TABLES"`

**Observe**:
- Output is `CREATE PUBLICATION`

**Awareness**:
- The name must be exactly `cache_server_pub` (PgPaw's default `--publication`). A typo means PgPaw replicates nothing and every query 400s as "not in publication".

**On weirdness**: abort

### Step 7: Launch PgPaw against the temp Postgres (background)

**Action**: `cargo run --release -- serve --pg-host 127.0.0.1 --pg-port 55437 --pg-user postgres --pg-password postgres --pg-database postgres --data-dir /tmp/pgpaw-redir-data --port 8085 --jwt-secret pgpaw-test-secret-please-change`

**Observe**:
- Process stays running (does not exit); startup logs show it connecting to the upstream and beginning replication
- A fresh `/tmp/pgpaw-redir-data` directory appears

**Awareness**:
- Run this in the background and capture stdout/stderr to a log the later Steps can tail. Confirm the bind line shows `127.0.0.1:8085`, not a different port picked up from a stale env var.
- The `run_in_background` tool's shell wrapper exits while the server child process stays alive. After launching, verify the server is running via `ps -p <PID>` and `lsof -i :8085` — a missing LISTEN line means the process truly exited; a present LISTEN line means it is running even if the shell wrapper reported completion.

**On weirdness**: abort

### Step 8: Poll until the replica catches up (run N times)

**Action**: `curl -s http://127.0.0.1:8085/healthz`

**Observe**:
- Body is JSON with `"status":"ok"` and a numeric `watermark`
- Re-run this Step until `status` is `ok` with a non-zero `watermark` (the replica has applied the seed); a few `halted`/zero-watermark polls right after launch are expected

**Awareness**:
- Record the `watermark` value from the first `ok` poll — Step 14's version-bump poll compares against it to tell "replica caught up" from "version did not bump".
- If `status` stays `halted`, read the `reason` — a halted replica makes every `/query` return 503, which is NOT a redirect-cache fault. Distinguish 503 (replica) from the 303/404 this test cares about.

**On weirdness**: retry once per poll; abort if still `halted` after several polls

### Step 9: Public query — expect a 303 redirect

**Action**: `curl -s -i -X POST http://127.0.0.1:8085/query -H 'content-type: application/json' -d '{"sql":"select id, name from items order by id"}'`

**Observe**:
- Status line is `303 See Other`
- A `Location:` header matches the shape `/q/{hash}/{version}` (hash and version are opaque tokens; only the two-segment shape is fixed)
- The 303 response itself carries `Cache-Control: no-store`

**Awareness**:
- Record the EXACT `Location` value (both the `{hash}` and `{version}` segments). It is the pre-insert snapshot URL reused in Steps 10, 11, and 15, and compared segment-by-segment in Step 14.
- A 401 here would mean `items` was wrongly classified private — re-check the PUBLIC grant (Step 5) and that RLS is OFF on `items`. A 400 "not in publication" points back to Step 6.

**On weirdness**: abort

### Step 10: Follow the snapshot URL — expect a cacheable 200

**Action**: `curl -s -i http://127.0.0.1:8085/q/<hash>/<version>` (substitute the Location captured in Step 9)

**Observe**:
- Status is `200 OK`
- Headers include `Cache-Control: public, max-age=259200` and an `ETag`
- Body is a JSON array of both items (alpha and beta)

**Awareness**:
- Record the exact `ETag` value — Step 12 re-fetches the same URL and asserts the ETag is unchanged.
- A `private, no-store` here would mean the public/private classifier mis-tagged this public table; the snapshot would not be CDN-cacheable.

**On weirdness**: note-and-continue

### Step 11: Idempotency — re-run the identical query

**Action**: `curl -s -i -X POST http://127.0.0.1:8085/query -H 'content-type: application/json' -d '{"sql":"select id, name from items order by id"}'`

**Observe**:
- Status line is `303 See Other`
- The `Location` is byte-for-byte the SAME `/q/{hash}/{version}` captured in Step 9 — same `{hash}` (content-addressed) AND same `{version}` (data unchanged)

**Awareness**:
- A DIFFERENT `{hash}` for identical SQL means fingerprinting is unstable — cache thrash, no CDN reuse. A different `{version}` with no upstream write means the version index is moving spuriously.
- Compare the full string, not just a prefix; the bug surfaces in either segment.

**On weirdness**: abort

### Step 12: ETag stability — re-follow the same snapshot URL

**Action**: `curl -s -i http://127.0.0.1:8085/q/<hash>/<version>` (the SAME Location from Step 9)

**Observe**:
- Status is `200 OK`
- The `ETag` header value is identical to the one recorded in Step 10
- Body is unchanged (alpha and beta)

**Awareness**:
- Do NOT expect a `304 Not Modified`; the cursor handler does not honor `If-None-Match`. The assertion is ETag PRESENCE and EQUALITY across identical fetches, not a conditional-GET 304.
- A changed ETag for the same snapshot key would mean the ETag is not derived from stable snapshot identity.

**On weirdness**: note-and-continue

### Step 13: Unknown cursor — expect 404 NotFound

**Action**: `curl -s -i http://127.0.0.1:8085/q/deadbeefdeadbeef/0`

**Observe**:
- Status is `404 Not Found`
- Body is a JSON envelope whose `name` is exactly `NotFound`
- The response returns promptly (no hang, no 500)

**Awareness**:
- The hash `deadbeefdeadbeef` was never produced, so this exercises the miss path. A `500` or a hang here means cursor error handling is broken — distinct from a legitimate 404.
- Confirm `Content-Type: application/json` on the 404 — a plaintext or HTML error body would indicate the error did not flow through the JSON envelope path.

**On weirdness**: abort

### Step 14: Upstream insert — bump the data

**Action**: `docker exec pgpaw-redir-pg psql -U postgres -c "INSERT INTO items VALUES (3,'gamma')"`

**Observe**:
- Output is `INSERT 0 1`

**Awareness**:
- This is an UPSTREAM write; PgPaw will not reflect it until logical replication applies it. Do not query PgPaw yet — Step 15 gates on the watermark first.
- A unique-violation error means id 3 already exists (container not fresh); abort.

**On weirdness**: abort

### Step 15: Poll until the watermark advances (run N times)

**Action**: `curl -s http://127.0.0.1:8085/healthz`

**Observe**:
- Body is JSON with `"status":"ok"` and a `watermark` that is strictly GREATER than the value recorded in Step 8
- Re-run this Step until the watermark has advanced past the Step-8 value

**Awareness**:
- This is the gate that separates "version did not bump" (a real invalidation bug) from "replica has not caught up yet" (just wait). Do NOT run Step 16 until the watermark has clearly advanced.
- If `status` flips to `halted` after the insert, read `reason` — a decode/apply error on the new row would halt replication and is a different failure than a stuck version.

**On weirdness**: retry once per poll; abort if the watermark never advances after several polls

### Step 16: Re-run the identical query — expect a bumped version

**Action**: `curl -s -i -X POST http://127.0.0.1:8085/query -H 'content-type: application/json' -d '{"sql":"select id, name from items order by id"}'`

**Observe**:
- Status line is `303 See Other`
- The `Location` `{version}` segment is DIFFERENT from (greater than) the pre-insert value captured in Step 9
- The `{hash}` segment may be unchanged — the SQL text is identical, so a content-addressed fingerprint legitimately stays the same; it is the `{version}` that must move

**Awareness**:
- Record this NEW `Location` for Step 17. Do not confuse a changed `{hash}` (would be surprising for identical SQL) with the expected changed `{version}`.
- If the `{version}` did NOT change even though Step 15 confirmed the watermark advanced, that is an invalidation bug (version_of not keying on the table write), NOT a replication lag.

**On weirdness**: abort

### Step 17: Follow the new snapshot URL — expect the new row

**Action**: `curl -s -i http://127.0.0.1:8085/q/<hash>/<version>` (substitute the NEW Location from Step 16)

**Observe**:
- Status is `200 OK` with `Cache-Control: public, max-age=259200` and an `ETag`
- Body is a JSON array of all three items — it now INCLUDES gamma (id 3) alongside alpha and beta

**Awareness**:
- The `ETag` here should differ from Step 10's (different snapshot key, different content). A matching ETag across the old and new snapshots would mean ETags are not snapshot-specific.

**On weirdness**: abort

### Step 18: Old snapshot immutability — re-follow the pre-insert URL

**Action**: `curl -s -i http://127.0.0.1:8085/q/<hash>/<version>` (the ORIGINAL pre-insert Location from Step 9)

**Observe**:
- Either: status `200 OK` and the body is the OLD content (alpha and beta only — NO gamma)
- Or: status `404 Not Found` with the `NotFound` envelope (the old snapshot was evicted from the bounded cache)

**Awareness**:
- A `200` whose body now INCLUDES gamma is the critical failure: content-addressed snapshots must be immutable, and a version bump must mint a NEW url without mutating the old one. A `404` is acceptable — the cache is bounded (moka `max_capacity`), so eviction of the old key is legitimate; only mutation of the old snapshot's body is a bug.

**On weirdness**: note-and-continue (404 is acceptable; a mutated old body is the abort-worthy case to flag)

## Expected Behavior

- A public query is served WITHOUT a token as `303 See Other` whose `Location` matches `/q/{hash}/{version}`; the redirect itself carries `Cache-Control: no-store`. Following that URL returns `200` with `Cache-Control: public, max-age=259200`, an `ETag`, and the rows — proving the snapshot is genuinely CDN-cacheable.
- Identical SQL is idempotent: the `{hash}` segment is content-addressed and stable across runs, and (with data unchanged) the full `/q/{hash}/{version}` Location is byte-identical. Re-fetching the same snapshot returns the same `ETag`; there is NO `304` (the cursor does not honor `If-None-Match`), only a stable `ETag` value.
- An unknown cursor (`/q/deadbeefdeadbeef/0`) returns `404` with a JSON envelope named `NotFound`, promptly, never a `500` or a hang.
- An upstream INSERT, once replication has applied it (watermark advanced on `/healthz`), bumps the `{version}` segment so the same SQL now redirects to a NEW snapshot URL whose body includes the new row (gamma). The version moves; the hash may legitimately stay the same.
- The pre-insert snapshot URL, if still cached, still returns the OLD immutable body (alpha, beta only); if it was evicted from the bounded cache it returns `404`. It must NEVER return a mutated body that includes gamma.

Reserve exact-match only for system-composed artifacts: the status codes (`303`, `200`, `404`), the header strings (`Cache-Control: public, max-age=259200`, `Cache-Control: no-store`), the `/q/{hash}/{version}` Location shape, and the `NotFound` envelope name. Row content is judged against the seed (alpha, beta, then gamma), not by exact byte match of the JSON.

## Fail Modes

- **Identical SQL yields a different `{hash}`** — unstable fingerprint → cache thrash, no CDN reuse. Check the classifier/fingerprint path; the hash must be a pure function of normalized SQL.
- **`{version}` does not bump after the insert** — invalidation not firing, or replica not caught up. First confirm Step 15 saw the watermark advance; if it did and the version is still stale, `version_of` is not keying on the `items` table write.
- **The pre-insert snapshot URL returns the NEW body (with gamma)** — snapshots are not immutable, content-addressing is violated. This is the critical, abort-worthy failure; a `404` (eviction) at the same URL is acceptable instead.
- **Unknown cursor returns `500` or hangs** — cursor miss-path error handling is broken; it must return the `NotFound` JSON envelope.
- **Follow returns `private, no-store` instead of `public, max-age=259200`** — the public/private classifier mis-tagged this public table; the snapshot would never be cacheable by a CDN.
- **Every query returns `503`** — replica halted or never caught up → re-read `/healthz` `reason`; this is a replication failure distinct from the redirect-cache behavior, so do not score the 303/404 steps until `/healthz` is `ok`.
- **No-token query returns `401`** — `items` was classified private; re-check the PUBLIC grant (Step 5) and that RLS is OFF on `items`.

## Cleanup

### Cleanup 1: Stop and remove the Postgres container

**Action**: `docker rm -f pgpaw-redir-pg`

**Observe**:
- Output prints the container name; `docker ps -a --filter name=pgpaw-redir-pg` is then empty

**Awareness**:
- A leftover container holds port 55437 and would break the next fresh run.

**On weirdness**: note-and-continue

### Cleanup 2: Stop the PgPaw server process

**Action**: `pkill -f '/tmp/pgpaw-redir-data'`

**Observe**:
- The background PgPaw process exits; port 8085 is freed (`lsof -i :8085` prints nothing)

**Awareness**:
- The process runs as `pgpaw serve`; match it by the unique `--data-dir` path `/tmp/pgpaw-redir-data` (a `cache-server.*serve` pattern would NOT match the actual binary name). If the socket enters `CLOSE_WAIT` after pkill, use `kill -9 <PID>` to force-terminate and free the port.

**On weirdness**: note-and-continue

### Cleanup 3: Remove the replica data directory

**Action**: `rm -rf /tmp/pgpaw-redir-data`

**Observe**:
- `/tmp/pgpaw-redir-data` no longer exists (`test -d /tmp/pgpaw-redir-data` returns non-zero)

**Awareness**:
- Reusing this dir on a later run reuses a stale replica and violates the FRESH WORKSPACE contract.

**On weirdness**: note-and-continue
