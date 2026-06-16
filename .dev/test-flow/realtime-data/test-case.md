---
id: realtime-data
title: A live /query?live=true stream pushes a follow-up SSE event when an upstream INSERT replicates in, proving live deltas flow end-to-end (not just that a stream opens)
tier: live
component: live
target: pgpaw (cache-server binary)
prerequisites:
  - "FRESH WORKSPACE REQUIRED — every live test MUST run against a freshly-created workspace (do NOT reuse a previously-tested one). A stale --data-dir replica or a leftover docker container poisons the version-bump and delta observations."
  - "`docker ps` succeeds (daemon reachable)"
  - "`curl --version` returns a version string"
  - "`cargo --version` returns a version string (PgPaw built from this repo, or `pgpaw` on PATH)"
  - "TCP port 55436 on 127.0.0.1 is free (`lsof -i :55436` prints nothing)"
  - "TCP port 8086 on 127.0.0.1 is free (`lsof -i :8086` prints nothing)"
  - "No container named `pgpaw-rt-pg` already exists (`docker ps -a --filter name=pgpaw-rt-pg --format '{{.Names}}'` prints nothing)"
  - "The data dir `/tmp/pgpaw-rt-data` does not yet exist (`test -d /tmp/pgpaw-rt-data` returns non-zero)"
expected_duration_secs: 360
tags: [live, sse, realtime, replication, deltas, cdc]
priority: high
created: 2026-06-16
author: senior-qa
---

## Objective

Verify that `POST /query?live=true` over a PUBLIC table opens a `text/event-stream`, emits an initial snapshot event, and then pushes a follow-up event after an upstream INSERT replicates in — and that the inserted row becomes queryable under a bumped `/q/{hash}/{version}`.

## Preconditions

- `docker ps` succeeds (daemon reachable)
- The table under test (`events`) is PUBLIC: RLS is OFF and `SELECT` is granted to PUBLIC, so live deltas are observable with no token
- The publication is named exactly `cache_server_pub` (PgPaw's default `--publication`)
- PgPaw is launched with `--jwt-secret pgpaw-test-secret-please-change` (unused by this public-only test, but it keeps the launch line identical to the auth suite)

## Inputs

JWT secret PgPaw is launched with (NOT exercised here — the whole flow runs token-free against a public table):

```text
pgpaw-test-secret-please-change
```

Schema + seed (PUBLIC table, no RLS) — applied one statement per Step in `## Steps`:

```sql
CREATE TABLE events (id int PRIMARY KEY, label text);
INSERT INTO events VALUES (1,'one'),(2,'two');
GRANT SELECT ON events TO PUBLIC;
CREATE PUBLICATION cache_server_pub FOR ALL TABLES;
```

Live query body (used for both the sanity open and the captured stream):

```json
{"sql":"select id, label from events order by id"}
```

Non-live query body (used for baseline + post-delta version checks):

```json
{"sql":"select * from events order by id"}
```

The upstream mutation injected mid-stream:

```sql
INSERT INTO events VALUES (3,'live-three')
```

## Steps

> **Notebook discipline**: Each Step is ONE action. The tester runs it, observes, judges, then moves to the next. No `&&`, `;`, multi-line shells, or for-loops inside a Step's Action. Polling = a Step the tester runs N times. The ONE sanctioned exception is the backgrounded SSE capture in Step 11, whose Action ends in a single trailing `&` to detach it — that is ONE async command, not command-chaining, and is flagged in that Step's Awareness.

### Step 1: Provision a fresh logical-replication Postgres

**Action**: `docker run -d --name pgpaw-rt-pg -e POSTGRES_PASSWORD=postgres -p 55436:5432 postgres:16 -c wal_level=logical`

**Observe**:
- Command exits 0 and prints a 64-hex container id
- `docker ps --filter name=pgpaw-rt-pg` shows the container as Up

**Awareness**:
- On first run this pulls `postgres:16` — a slow pull is normal, not a failure. A non-zero exit citing "port is already allocated" means port 55436 was not actually free; abort and re-check the precondition.
- Confirm no OTHER postgres container is bound to 55436 that could shadow this one.

**On weirdness**: abort

### Step 2: Confirm wal_level is logical

**Action**: `docker exec pgpaw-rt-pg psql -U postgres -tAc "SHOW wal_level"`

**Observe**:
- Output is exactly `logical`

**Awareness**:
- If this prints `replica` the `-c wal_level=logical` flag did not take — logical replication never starts and the live stream will only ever show the snapshot, never a delta. Treat a wrong value as a hard stop.
- A "FATAL: the database system is starting up" error means Postgres is not ready yet; wait a few seconds and retry.

**On weirdness**: retry once (for startup race); abort if value is not `logical`

### Step 3: Create the PUBLIC events table

**Action**: `docker exec pgpaw-rt-pg psql -U postgres -c "CREATE TABLE events (id int PRIMARY KEY, label text)"`

**Observe**:
- Output is `CREATE TABLE`

**Awareness**:
- Watch for any NOTICE about an existing relation — that would mean the container is not actually fresh.
- The PRIMARY KEY on `id` matters: the live engine keys its delta diff on the single-column PK, so a missing PK would change how (and whether) an insert surfaces as a keyed delta.

**On weirdness**: abort

### Step 4: Seed the initial rows

**Action**: `docker exec pgpaw-rt-pg psql -U postgres -c "INSERT INTO events VALUES (1,'one'),(2,'two')"`

**Observe**:
- Output is `INSERT 0 2`

**Awareness**:
- A row count other than 2 means a partial seed; the initial snapshot event then reflects the wrong baseline and the delta judgment becomes unreliable.

**On weirdness**: abort

### Step 5: Grant public read on events

**Action**: `docker exec pgpaw-rt-pg psql -U postgres -c "GRANT SELECT ON events TO PUBLIC"`

**Observe**:
- Output is `GRANT`

**Awareness**:
- This is what classifies `events` PUBLIC. Without it (and with RLS off) the table would be treated as access-controlled and `?live=true` would return `403`, not a stream — the whole realtime path would be untestable token-free.

**On weirdness**: abort

### Step 6: Create the publication PgPaw replicates

**Action**: `docker exec pgpaw-rt-pg psql -U postgres -c "CREATE PUBLICATION cache_server_pub FOR ALL TABLES"`

**Observe**:
- Output is `CREATE PUBLICATION`

**Awareness**:
- The name must be exactly `cache_server_pub` (PgPaw's default `--publication`). A typo means PgPaw replicates nothing — both the initial snapshot AND any delta will be empty/absent.

**On weirdness**: abort

### Step 7: Launch PgPaw against the temp Postgres (background)

**Action**: `cargo run --release -- serve --pg-host 127.0.0.1 --pg-port 55436 --pg-user postgres --pg-password postgres --pg-database postgres --data-dir /tmp/pgpaw-rt-data --port 8086 --jwt-secret pgpaw-test-secret-please-change`

**Observe**:
- Process stays running (does not exit); startup logs show it connecting upstream and beginning replication
- A fresh `/tmp/pgpaw-rt-data` directory appears

**Awareness**:
- Run this in the background and capture stdout/stderr to a log later Steps can tail. Confirm the bind line shows `127.0.0.1:8086`, not a port picked up from a stale env var.
- The `run_in_background` tool's shell wrapper exits while the server child stays alive. After launching, verify via `ps -p <PID>` and `lsof -i :8086` — a missing LISTEN line means the process truly exited; a present LISTEN line means it is running even if the wrapper reported completion.

**On weirdness**: abort

### Step 8: Poll until the replica catches up (run N times)

**Action**: `curl -s http://127.0.0.1:8086/healthz`

**Observe**:
- Body is JSON with `"status":"ok"` and a numeric `watermark`
- Re-run this Step until `status` is `ok` with a non-zero watermark; a few `halted`/zero-watermark polls right after launch are expected

**Awareness**:
- If `status` stays `halted`, read the `reason` — a halted replica makes every `/query` return 503, which is NOT a realtime failure. Distinguish 503 (replica) from a missing delta (live engine).
- Do not capture the baseline (Step 10) or open the stream until at least one `ok` poll is seen, or the snapshot will reflect a not-yet-replicated table.

**On weirdness**: retry once per poll; abort if still `halted` after several polls

### Step 9: Sanity — public live stream opens as event-stream

**Action**: `curl -s -N -i --max-time 5 -X POST 'http://127.0.0.1:8086/query?live=true' -H 'content-type: application/json' -d '{"sql":"select id, label from events order by id"}'`

**Observe**:
- Status is `200 OK` with `Content-Type: text/event-stream`
- Header includes `Cache-Control: no-store`
- At least one `data:` event is emitted (the opening snapshot pointer); the connection stays open until `--max-time` closes it

**Awareness**:
- This is a streaming endpoint — the `--max-time 5` cap is mandatory so the Step cannot hang. The content type must be `text/event-stream`, NOT `application/json` and NOT a `303`; either of those means live mode was ignored or mis-routed.
- The opening event is a snapshot pointer (carries a `/q/{hash}/{version}` url and a `version`), not row data — that shape is expected.

**On weirdness**: note-and-continue

### Step 10: Capture the PRE-insert version baseline

**Action**: `curl -s -i -X POST http://127.0.0.1:8086/query -H 'content-type: application/json' -d '{"sql":"select * from events order by id"}'`

**Observe**:
- Status is `303 See Other`
- A `Location:` header matches the shape `/q/{hash}/{version}`; record the exact `{version}` segment as the BASELINE for Step 14

**Awareness**:
- Note this version verbatim — Step 14 proves it BUMPED after the insert. If you skip recording it here, the later bump comparison is meaningless.
- The `303` carries `Cache-Control: no-store` (the long-lived `public, max-age` lives on the followed `/q/...` fetch, not the redirect).

**On weirdness**: abort

### Step 11: CAPTURE — background an ~8s stream to a file

**Action**: `curl -s -N --max-time 8 -X POST 'http://127.0.0.1:8086/query?live=true' -H 'content-type: application/json' -d '{"sql":"select id, label from events order by id"}' -o /tmp/rt-stream.log &`

**Observe**:
- The command returns immediately (backgrounded); a job/PID is printed
- `/tmp/rt-stream.log` is created and begins filling

**Awareness**:
- The trailing `&` is the ONE sanctioned async exception — it backgrounds a SINGLE curl, it is NOT command-chaining. Do not add `&&` or a second command.
- The `--max-time 8` is the capture window. The mutate (Step 12) MUST be run promptly, WITHIN this 8-second window, or the insert lands after the stream closes and no delta is captured.
- Note the start time so you know roughly how much of the 8s window remains when you fire Step 12.

**On weirdness**: retry once (re-run this Step to reopen the window)

### Step 12: MUTATE — insert a new row upstream during the window

**Action**: `docker exec pgpaw-rt-pg psql -U postgres -c "INSERT INTO events VALUES (3,'live-three')"`

**Observe**:
- Output is `INSERT 0 1`

**Awareness**:
- This MUST execute while the Step 11 window is still open (within ~8s of starting the capture). If you were too slow, expect only a snapshot in Step 13 — re-run the trio (Steps 11→12→13) before concluding a fail; that is timing, not necessarily a bug.
- The id `3` is new (not 1 or 2) so the delta is unambiguously an insert, not a no-op re-emit of the baseline.

**On weirdness**: retry once (re-run the 11→12→13 trio)

### Step 13: READ — inspect the captured stream

**Action**: `cat /tmp/rt-stream.log`

**Observe**:
- An initial snapshot event appears (a `data:` line carrying a snapshot pointer with a `/q/{hash}/{version}` url and a `version`)
- A LATER event appears AFTER it, triggered by the insert — a `data:` line conveying the new row (an insert delta referencing id `3` / `live-three`), and/or an up-to-date marker following the delta

**Awareness**:
- Judge MEANING, not exact bytes: the second event proves a delta propagated. Wording/field order of the delta JSON may vary; what matters is that a post-snapshot event exists and references the inserted row.
- If the file shows ONLY the snapshot and no later event, the insert likely landed outside the window — re-run the 11→12→13 trio once before judging a fail. If it STILL shows snapshot-only after a clean in-window mutate, re-check `/healthz` watermark advanced past the insert (replication may not be reaching the live subscriber).

**On weirdness**: retry once (re-run the 11→12→13 trio); then note-and-continue if still snapshot-only

### Step 14: Post-delta version check — confirm the bump

**Action**: `curl -s -i -X POST http://127.0.0.1:8086/query -H 'content-type: application/json' -d '{"sql":"select * from events order by id"}'`

**Observe**:
- Status is `303 See Other`
- The `Location:` `/q/{hash}/{version}` carries a version that is BUMPED relative to the Step 10 baseline (different/higher version segment)

**Awareness**:
- The hash may stay the same (same SQL shape) while the version changes — it is the `{version}` segment that must differ from the Step 10 baseline. If the version is identical, the insert did not bump the table version (version_of / invalidation not firing).
- Record the new `Location` to follow in Step 15.

**On weirdness**: abort

### Step 15: Follow the bumped snapshot URL

**Action**: `curl -s -i http://127.0.0.1:8086/q/<hash>/<version>` (substitute the Location captured in Step 14)

**Observe**:
- Status is `200 OK` with `Cache-Control: public, max-age=259200` and an `ETag`
- Body is a JSON array that now INCLUDES the inserted row (id `3`, label `live-three`) alongside ids 1 and 2

**Awareness**:
- This confirms the realtime change is also content-addressed and queryable under the new version, not just streamed. A body missing id `3` here means the replicated insert never reached the materialized snapshot — cross-check `/healthz` watermark.
- A `private, no-store` here instead of `public, max-age=259200` would mean the public/private classifier mis-tagged `events`.

**On weirdness**: note-and-continue

## Expected Behavior

- `?live=true` over the public `events` table returns `200` with `Content-Type: text/event-stream` and `Cache-Control: no-store`, never `application/json` and never a `303`. Returning JSON or a redirect would mean live mode was ignored or mis-routed.
- The stream emits an OPENING snapshot event (a `data:` line carrying a `/q/{hash}/{version}` pointer and a `version`) and the connection stays open.
- After an upstream `INSERT` replicates in, the SAME open stream emits an ADDITIONAL event referencing the new row (an insert delta for id `3` / `live-three`, optionally followed by an up-to-date marker) — the delta actually propagates end-to-end. Judge this by MEANING; the delta JSON wording, field order, and exact framing are behavioral, not byte-exact.
- A non-live `select * from events` taken AFTER the insert redirects (`303`) to a `/q/{hash}/{version}` whose version is BUMPED relative to the pre-insert baseline captured in Step 10; following that URL returns `200` with `Cache-Control: public, max-age=259200` and a body that now includes id `3`.
- Throughout, `/healthz` reports `status: ok` with a monotonic watermark that advances past the insert.

Reserve exact-match only for system-composed artifacts: the status codes (`200`, `303`), the header strings (`Content-Type: text/event-stream`, `Cache-Control: no-store`, `Cache-Control: public, max-age=259200`), the `/q/{hash}/{version}` Location shape, and the SSE `data:` framing. Whether a delta event appears (and that it references the inserted row) is judged behaviorally; the delta payload's exact JSON is not.

## Fail Modes

- **Live request returns `application/json` or a `303` instead of `text/event-stream`** — live mode ignored or mis-routed, or `events` was wrongly classified private (which yields `403`) → confirm RLS is off and `SELECT` is granted to PUBLIC (Step 5), and that the `?live=true` query param actually reached the handler.
- **Only the snapshot event ever appears, never a delta** — most often the insert landed outside the 8s capture window → re-run the 11→12→13 trio with a faster mutate. If it persists with a clean in-window mutate, replication is not reaching the live subscriber → check `/healthz` watermark advanced past the insert, and that the publication is `cache_server_pub FOR ALL TABLES`.
- **Version does not bump after the insert** — `version_of` / invalidation not firing on the CDC commit → confirm the insert committed upstream (`INSERT 0 1`) and that the watermark moved; compare the Step 14 `{version}` against the Step 10 baseline exactly.
- **The followed `/q/...` body is missing id `3`** — the replicated insert never reached the materialized snapshot → cross-check the watermark and that Step 14's version is genuinely the post-insert one (not a re-read of the stale baseline URL).
- **The stream Step hangs indefinitely** — missing `--max-time` cap → every SSE read in this notebook MUST be capped (`--max-time` on the foreground sanity read, and on the backgrounded capture).
- **All queries `503`** — replica halted or never caught up → re-read `/healthz` `reason`; this is a replication failure distinct from realtime behavior, so do not score the stream steps until `/healthz` is `ok`.

## Cleanup

### Cleanup 1: Stop and remove the Postgres container

**Action**: `docker rm -f pgpaw-rt-pg`

**Observe**:
- Output prints the container name; `docker ps -a --filter name=pgpaw-rt-pg` is then empty

**Awareness**:
- A leftover container holds port 55436 and would break the next fresh run.

**On weirdness**: note-and-continue

### Cleanup 2: Stop the PgPaw server process

**Action**: `pkill -f '/tmp/pgpaw-rt-data'`

**Observe**:
- The background PgPaw process exits; port 8086 is freed (`lsof -i :8086` prints nothing)

**Awareness**:
- Kill by the unique data-dir path — the process runs as `pgpaw serve`, so a `cache-server.*serve` pattern does NOT match it. Matching on `/tmp/pgpaw-rt-data` avoids killing an unrelated pgpaw instance.
- If the socket enters `CLOSE_WAIT` after pkill, use `kill -9 <PID>` to force-terminate and free the port.

**On weirdness**: note-and-continue

### Cleanup 3: Remove the replica data directory

**Action**: `rm -rf /tmp/pgpaw-rt-data`

**Observe**:
- `/tmp/pgpaw-rt-data` no longer exists (`test -d /tmp/pgpaw-rt-data` returns non-zero)

**Awareness**:
- Reusing this dir on a later run reuses a stale replica and violates the FRESH WORKSPACE contract.

**On weirdness**: note-and-continue

### Cleanup 4: Remove the captured stream file

**Action**: `rm -f /tmp/rt-stream.log`

**Observe**:
- `/tmp/rt-stream.log` no longer exists (`test -f /tmp/rt-stream.log` returns non-zero)

**Awareness**:
- A leftover stream log from a prior run would mislead a re-read of Step 13 into judging stale content.

**On weirdness**: note-and-continue
