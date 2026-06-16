---
id: example-nextjs-todos
title: The nextjs-todos example, wired through @pgpaw/tanstack-db to a live PgPaw server, performs an end-to-end optimistic CRUD round-trip and receives a live SSE delta from a direct upstream UPDATE without a page reload
tier: live
component: examples
target: examples/nextjs-todos
prerequisites:
  - "FRESH WORKSPACE REQUIRED — this is a LIVE test. It MUST run against a freshly-created Postgres (no leftover `todos` rows) and with NO stale PgPaw serve / Next dev server / docker container still bound to ports 8080, 3000, or 5432. Prior on-disk state or a stale process poisons the snapshot, the version-bump, and the live-delta observations."
  - "`docker ps` succeeds (daemon reachable)"
  - "`test -x target/release/pgpaw` returns 0 (release binary already built)"
  - "`pnpm --version` returns a version string"
  - "`curl --version` returns a version string"
  - "TCP port 5432 on 127.0.0.1 is free (`lsof -i :5432` prints nothing)"
  - "TCP port 8080 on 127.0.0.1 is free (`lsof -i :8080` prints nothing)"
  - "TCP port 3000 on 127.0.0.1 is free (`lsof -i :3000` prints nothing)"
  - "No container named `pgpaw-todos-pg` already exists (`docker ps -a --filter name=pgpaw-todos-pg --format '{{.Names}}'` prints nothing)"
  - "`test -f examples/nextjs-todos/schema.sql` returns 0"
expected_duration_secs: 900
tags: [examples, tanstack-db, nextjs, live-sync]
author: senior-qa
priority: high
created: 2026-06-16
---

## Objective

Verify that the `examples/nextjs-todos` app, driven by `@pgpaw/tanstack-db` against a live PgPaw server over real Postgres logical replication, completes an optimistic CRUD round-trip (add, toggle, delete reflected in both UI and DB) and applies a live SSE delta — a direct psql `UPDATE` changes the rendered row text without a page reload.

## Preconditions

- `docker ps` succeeds (daemon reachable)
- `todos` is a SINGLE-TABLE PUBLIC collection: no auth, `SELECT` granted to PUBLIC, so the live first event is a `/q/{hash}/{version}` snapshot pointer the adapter fetches token-free
- The release binary at `target/release/pgpaw` was built from this repo
- The Playwright MCP server is connected (if not, the UI Steps fall back to curl-ing the example's API + PgPaw directly — see the per-Step Awareness)

## Inputs

PgPaw init / serve connection (one upstream Postgres, db `myapp`):

```text
--pg-host 127.0.0.1 --pg-user postgres --pg-password postgres --pg-database myapp
```

Example env (copied from `.env.example` to `.env`):

```text
NEXT_PUBLIC_PGPAW_URL=http://127.0.0.1:8080
DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/myapp
```

Browser entrypoint:

```text
http://localhost:3000
```

Todo title typed in the UI add-flow:

```text
buy milk
```

The mid-session live mutation injected directly in psql (substitute `<id>` with the id read from the DB in Step 19):

```sql
update todos set title='changed' where id='<id>';
```

DB read used to verify persistence and live state:

```sql
select id, title, completed from todos;
```

## Steps

> **Notebook discipline**: Each Step is ONE action. The tester runs it, observes, judges, then moves to the next. No `&&`, `;`, multi-line shells, or for-loops inside a Step's Action. Polling = a Step the tester runs N times. Long-running servers (Postgres, `pgpaw serve`, `pnpm dev`) are launched in the background as their own single-command Steps.

### Step 1: Provision a fresh logical-replication Postgres

**Action**: `docker run -d --name pgpaw-todos-pg -e POSTGRES_PASSWORD=postgres -e POSTGRES_DB=myapp -p 5432:5432 postgres:16 -c wal_level=logical`

**Observe**:
- Command exits 0 and prints a 64-hex container id
- `docker ps --filter name=pgpaw-todos-pg` then shows the container Up

**Awareness**:
- A non-zero exit citing "port is already allocated" means 5432 was not actually free — abort and recheck the precondition. A first-run `postgres:16` pull is slow but normal, not a failure.
- Confirm no OTHER postgres (local install or another container) is shadowing 5432.

**On weirdness**: abort

### Step 2: Poll until Postgres accepts connections (run N times)

**Action**: `docker exec pgpaw-todos-pg pg_isready -U postgres`

**Observe**:
- Output ends in `accepting connections`
- Re-run this Step until it reports accepting; a few "no response" / "starting up" polls right after Step 1 are expected

**Awareness**:
- This is a polling Step, not a one-shot — do NOT proceed to schema load until you see `accepting connections`, or the `psql -f` in Step 4 races the boot and fails.
- A persistent refusal after many polls suggests the container crashed; check `docker ps` shows it still Up, not Exited.

**On weirdness**: retry once per poll; abort if still refusing after several polls

### Step 3: Confirm wal_level is logical

**Action**: `docker exec pgpaw-todos-pg psql -U postgres -d myapp -tAc "SHOW wal_level"`

**Observe**:
- Output is exactly `logical`

**Awareness**:
- If this prints `replica`, the `-c wal_level=logical` flag did not take — logical replication never starts, PgPaw will `503 halted`, and NO live delta (Step 20) can ever arrive. Treat a wrong value as a hard stop, not a flaky retry.

**On weirdness**: abort

### Step 4: Load the todos schema

**Action**: `psql postgresql://postgres:postgres@127.0.0.1:5432/myapp -f examples/nextjs-todos/schema.sql`

**Observe**:
- Output shows `CREATE TABLE` and `GRANT` (the schema creates `todos` and grants `SELECT` to PUBLIC)

**Awareness**:
- The schema declares `id text primary key` — the id is application-generated, NOT a Postgres `uuid` default. Later Steps expect a non-empty stable id STRING, not necessarily a uuid; do not fail the test merely because the id is not uuid-shaped.
- The `GRANT SELECT ... TO PUBLIC` is what classifies `todos` as a public collection; without it the live path would be access-controlled and the snapshot pointer would not be reachable token-free.

**On weirdness**: abort

### Step 5: Initialize PgPaw against the upstream

**Action**: `target/release/pgpaw init --pg-host 127.0.0.1 --pg-user postgres --pg-password postgres --pg-database myapp`

**Observe**:
- Command exits 0 and reports it set up the cache server / created the publication `cache_server_pub`

**Awareness**:
- `init` creates the publication `cache_server_pub` but does NOT necessarily include `todos` yet — that is Step 6. Note whether init already added the table so Step 6's "already member" notice is interpreted correctly.

**On weirdness**: abort

### Step 6: Add todos to the publication

**Action**: `psql postgresql://postgres:postgres@127.0.0.1:5432/myapp -c "alter publication cache_server_pub add table todos"`

**Observe**:
- Output is `ALTER PUBLICATION`, OR a notice that `todos` is already a member of `cache_server_pub`

**Awareness**:
- An "already a member" / "relation is already in publication" notice is FINE (init may have included it) — note-and-continue, do not abort. What matters is that after this Step `todos` is in `cache_server_pub`; if it is NOT, live deltas (Step 20) will never fire.

**On weirdness**: note-and-continue

### Step 7: Launch PgPaw serve (background)

**Action**: `target/release/pgpaw serve --pg-host 127.0.0.1 --pg-user postgres --pg-password postgres --pg-database myapp --port 8080`

**Observe**:
- Process stays running (does not exit); startup logs show it connecting upstream and beginning replication

**Awareness**:
- Run in the background and capture stdout/stderr to a log later Steps can read. The background shell wrapper may report "completed" while the server child stays alive — verify with `lsof -i :8080` showing a LISTEN line, not by trusting the wrapper.
- Confirm the bind shows `127.0.0.1:8080`, not a port from a stale env var.

**On weirdness**: abort

### Step 8: Poll PgPaw health until ok (run N times)

**Action**: `curl -s http://127.0.0.1:8080/healthz`

**Observe**:
- Body is JSON with `"status":"ok"` and a NUMERIC `watermark`
- Re-run until `status` is `ok` with a non-zero watermark; a few `halted` / zero-watermark polls right after launch are expected

**Awareness**:
- If `status` stays `halted`, read `reason`. A halted replica makes every `/query` (and the example's API) return `503` — that is a replication failure, distinct from any UI bug. Do NOT start the browser flow until at least one `ok` poll is seen.
- `503 halted` after a clean `logical` wal_level usually means the replica slot did not start; cross-check Steps 3 and 6.

**On weirdness**: retry once per poll; abort if still `halted` after several polls

### Step 9: Install tanstack-db deps (ignore workspace)

**Action**: `pnpm install --ignore-workspace --dir packages/tanstack-db`

**Observe**:
- Install completes; `packages/tanstack-db/node_modules` is populated

**Awareness**:
- `--ignore-workspace` is REQUIRED. Without it, the parent `a-zero` pnpm workspace hijacks resolution and the package resolves against the monorepo instead of standalone. If you see the install pulling in unrelated a-zero workspace packages, the flag was dropped — abort and rerun with the flag.

**On weirdness**: retry once (re-run WITH `--ignore-workspace`)

### Step 10: Build tanstack-db

**Action**: `pnpm --dir packages/tanstack-db build`

**Observe**:
- tsup completes; `packages/tanstack-db/dist/` contains `index.js`, `index.cjs`, and `index.d.ts`

**Awareness**:
- The example declares `@pgpaw/tanstack-db: ^0.1.0` (a registry-style spec, NOT a `file:` path), so a missing `dist/` is what produces the "cannot resolve / blank import" failure in the browser later. Confirm `dist/` is non-empty before linking.

**On weirdness**: abort

### Step 11: Link tanstack-db globally

**Action**: `pnpm --dir packages/tanstack-db link --global`

**Observe**:
- Command reports the global link for `@pgpaw/tanstack-db` was created

**Awareness**:
- This registers the just-built `dist/` under the global pnpm store. If a PRIOR run's global link still exists, it may point at stale `dist/` — re-linking after the Step 10 rebuild overwrites it, which is what we want.

**On weirdness**: abort

### Step 12: Install example deps (ignore workspace)

**Action**: `pnpm install --ignore-workspace --dir examples/nextjs-todos`

**Observe**:
- Install completes; `examples/nextjs-todos/node_modules` is populated (next, react, @tanstack/*)

**Awareness**:
- Same `--ignore-workspace` rule as Step 9. The example is NOT part of the published workspace graph here; dropping the flag drags in the parent monorepo and the link in Step 13 will not stick.

**On weirdness**: retry once (re-run WITH `--ignore-workspace`)

### Step 13: Link tanstack-db into the example

**Action**: `pnpm --dir examples/nextjs-todos link --global @pgpaw/tanstack-db`

**Observe**:
- Command completes; `examples/nextjs-todos/node_modules/@pgpaw/tanstack-db` resolves to the linked package (a symlink into the global store)

**Awareness**:
- After this, `node_modules/@pgpaw/tanstack-db` should be a symlink, not a copied registry package. If it is a real directory with an OLD version, the link did not replace the registry resolution — that surfaces as a stale or blank import in the browser.

**On weirdness**: abort

### Step 14: Create the example .env

**Action**: `cp examples/nextjs-todos/.env.example examples/nextjs-todos/.env`

**Observe**:
- `examples/nextjs-todos/.env` now exists with `NEXT_PUBLIC_PGPAW_URL=http://127.0.0.1:8080` and `DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/myapp`

**Awareness**:
- `NEXT_PUBLIC_PGPAW_URL` must point at the Step 7 port (8080); a mismatch means the browser collection talks to nothing and the list never hydrates. `DATABASE_URL` must point at the Step 1 Postgres (5432/myapp); a mismatch means the Next route handler writes to the wrong DB and psql verification will not see the row.

**On weirdness**: abort

### Step 15: Launch the example dev server (background)

**Action**: `pnpm --dir examples/nextjs-todos dev`

**Observe**:
- Next.js compiles and prints a "ready" line bound to http://localhost:3000

**Awareness**:
- Run in the background. As in Step 7, verify the listener with `lsof -i :3000` rather than trusting the wrapper. A compile error mentioning `@pgpaw/tanstack-db` here (not at browse-time) means the link/build (Steps 10–13) is broken — go back, do not browse.

**On weirdness**: abort

### Step 16: Navigate the browser to the app

**Action**: `browser_navigate http://localhost:3000`

**Observe**:
- The page loads without a fatal error overlay; the todos UI shell renders

**Awareness**:
- Requires the Playwright MCP server connected. If it is NOT, FALL BACK to verifying the API directly with curl (e.g. the example's todos GET route and PgPaw `/healthz`) and treat the remaining browser Steps as curl-against-API equivalents — note the fallback in the result.
- A Next.js red error overlay mentioning a missing `@pgpaw/tanstack-db` export is the "import didn't resolve" fail mode, not an empty-list state.

**On weirdness**: note-and-continue (switch to curl fallback)

### Step 17: Snapshot the initial list state

**Action**: `browser_snapshot`

**Observe**:
- The accessibility tree shows the todo list region — either empty or rendering whatever rows exist (the DB starts empty after a fresh Step 4, so an empty list is the healthy baseline)
- An add input/control and an Add affordance are present

**Awareness**:
- Confirm the collection actually HYDRATED (the list region exists), versus a perpetual loading spinner — a stuck spinner means the live `POST /query?live=true` never resolved its `/q/{hash}/{version}` snapshot. Distinguish "empty list" (healthy) from "never loaded" (broken).

**On weirdness**: note-and-continue

### Step 18: Type a new todo title

**Action**: `browser_type` the text `buy milk` into the todo title input

**Observe**:
- The input field reflects the typed text `buy milk`

**Awareness**:
- This is a single field-fill, NOT a submit — no row should appear yet and no DB write should have happened. If a row appears merely from typing, the submit wiring is firing on keystroke (a bug worth noting).

**On weirdness**: retry once

### Step 19: Submit the new todo (click Add)

**Action**: `browser_click` the Add button

**Observe**:
- A new row reading `buy milk` appears in the list IMMEDIATELY (optimistic insert), before any obvious round-trip delay
- The input clears / resets

**Awareness**:
- The appearance should be optimistic (instant). Watch whether the row then briefly disappears and reappears or flips to an error state — a vanish-after-insert means the server write was rejected and the optimistic insert rolled back (the route handler or txid confirmation failed), which is different from a healthy instant-and-stays insert.

**On weirdness**: note-and-continue

### Step 20: Verify the insert persisted in Postgres

**Action**: `psql postgresql://postgres:postgres@127.0.0.1:5432/myapp -c "select id, title, completed from todos"`

**Observe**:
- Exactly one row with `title = buy milk`, `completed = f`, and a non-empty stable `id` string
- Record the `id` value verbatim — Step 23 needs it for the live UPDATE

**Awareness**:
- The optimistic UI insert (Step 19) is NOT proof of persistence — this psql read is. If the UI shows the row but psql shows zero rows, the optimistic write never confirmed against the DB (txid round-trip failed) even though the UI looked fine.
- The id is a `text` PK (app-generated); do not reject it for not being uuid-shaped.

**On weirdness**: abort

### Step 21: Toggle the todo completed in the UI

**Action**: `browser_click` the `buy milk` row's completed checkbox

**Observe**:
- The checkbox flips to checked and the row reflects a completed state in the UI

**Awareness**:
- Confirm the toggle STICKS (does not flip back). A flip-back signals the toggle write was rejected and rolled back, not applied.

**On weirdness**: note-and-continue

### Step 22: Verify the toggle persisted in Postgres

**Action**: `psql postgresql://postgres:postgres@127.0.0.1:5432/myapp -c "select id, title, completed from todos"`

**Observe**:
- The `buy milk` row now shows `completed = t`

**Awareness**:
- If the UI shows checked but psql still shows `completed = f`, the toggle was optimistic-only and never confirmed — the same txid-confirmation failure mode as Step 20, surfaced on UPDATE instead of INSERT.

**On weirdness**: abort

### Step 23: LIVE — change the title directly in Postgres

**Action**: `psql postgresql://postgres:postgres@127.0.0.1:5432/myapp -c "update todos set title='changed' where id='<id>'"` (substitute `<id>` recorded in Step 20)

**Observe**:
- Output is `UPDATE 1`

**Awareness**:
- This write bypasses the browser entirely — it is the live-propagation trigger. If you substituted the wrong id, you get `UPDATE 0` and Step 24 will (correctly) see no change; verify `UPDATE 1` before judging Step 24.

**On weirdness**: retry once (re-check the id from Step 20)

### Step 24: Observe the live delta in the browser (no reload)

**Action**: `browser_snapshot`

**Observe**:
- Within a few seconds, WITHOUT any page reload or browser navigation, the row that read `buy milk` now reads `changed`
- No full-page refresh occurred between Step 23 and this snapshot

**Awareness**:
- This is THE live-sync assertion: the change arrived as an SSE delta, not via a reload. If the text only updates after you manually reload, that is NOT live propagation — note it as a live-path failure even though the data is correct.
- Allow a few seconds of replication lag before judging a fail; re-snapshot once after waiting if the first snapshot still shows the old text. Cross-check `/healthz` watermark advanced past the update.

**On weirdness**: retry once (re-snapshot after a short wait); then note-and-continue

### Step 25: Confirm the live request shape (network awareness)

**Action**: `browser_snapshot` of the network/requests (or re-issue `curl -s -i -N --max-time 5 -X POST 'http://127.0.0.1:8080/query?live=true' -H 'content-type: application/json' -d '{"sql":"select id, title, completed from todos"}'` if network introspection is unavailable)

**Observe**:
- A live request to `POST /query?live=true` is present and returns `Content-Type: text/event-stream`
- The snapshot for this public collection is fetched via a `GET /q/{hash}/{version}` pointer path

**Awareness**:
- The `/q/{hash}/{version}` path shape is a SYSTEM-composed artifact — exact-match the path SHAPE (hash segment then version segment), but the actual hash/version values are runtime-specific and must not be hard-coded.
- A live request returning `application/json` or a `303` instead of `text/event-stream` means live mode was ignored or mis-routed.

**On weirdness**: note-and-continue

### Step 26: Delete the todo in the UI

**Action**: `browser_click` the `changed` row's delete control

**Observe**:
- The row disappears from the list immediately (optimistic delete)

**Awareness**:
- Confirm the row stays gone and does not re-appear (which would indicate the delete was rejected and rolled back).

**On weirdness**: note-and-continue

### Step 27: Verify the delete persisted in Postgres

**Action**: `psql postgresql://postgres:postgres@127.0.0.1:5432/myapp -c "select id, title, completed from todos"`

**Observe**:
- Zero rows (the `changed` row is gone from the DB)

**Awareness**:
- If the UI list is empty but psql still returns the row, the delete was optimistic-only and never confirmed against the DB — the delete-side counterpart to the Step 20/22 confirmation failure.

**On weirdness**: abort

## Expected Behavior

- PgPaw `/healthz` reports `status: ok` with a numeric, monotonically advancing `watermark`; a transient `halted` only right after launch is acceptable, a persistent one is not.
- The app at http://localhost:3000 loads without a fatal/import error and renders the todo list region — empty after a fresh DB is the healthy baseline, not a failure.
- Adding a todo shows the row in the UI IMMEDIATELY (optimistic), and a subsequent psql `select` shows a single matching row with a non-empty stable `id` string, `title = buy milk`, `completed = f`. The UI appearance and the DB persistence are SEPARATE assertions — both must hold.
- Toggling the checkbox flips `completed` in the UI and the change is confirmed in psql (`completed = t`).
- A direct psql `UPDATE ... set title='changed'` causes the rendered row text to change from `buy milk` to `changed` WITHOUT a page reload, within a few seconds — proving an SSE delta propagated end-to-end (upstream → replication → PgPaw live → adapter → UI). Judge by MEANING (the text changed live), not by exact SSE bytes or timing.
- Deleting the todo removes it from the UI and from psql (zero rows remain).
- The live data flow issues `POST /query?live=true` returning `Content-Type: text/event-stream`, and the public snapshot is fetched via a `GET /q/{hash}/{version}` pointer.

Reserve exact-match only for system-composed artifacts: HTTP status codes, the header string `Content-Type: text/event-stream`, the `/query?live=true` route, and the `/q/{hash}/{version}` PATH SHAPE. The todo `id` value, the SSE delta payload wording, UI element ordering, and exact timing are behavioral and must not be byte-matched.

## Fail Modes

- **Browser shows a blank page or a Next overlay citing a missing `@pgpaw/tanstack-db` export** — the build/link sequence (Steps 10–13) was skipped or the global link points at stale/empty `dist/` → confirm `packages/tanstack-db/dist/` is non-empty and that `examples/nextjs-todos/node_modules/@pgpaw/tanstack-db` is a SYMLINK into the global store. Fallback documented in the example README: replace the dependency with `file:../../packages/tanstack-db` and re-run `pnpm install --ignore-workspace`.
- **All `/query` and the example API return `503 halted`** — the logical replica never started, usually because `wal_level` is `replica` not `logical` → re-check Step 3; a wrong wal_level is a hard stop, recreate the container with `-c wal_level=logical`.
- **CRUD works in UI and DB but the Step 24 live UPDATE never appears without a reload** — `todos` is not in the publication, so no delta is emitted → re-run Step 6 and confirm `todos` is a member of `cache_server_pub`; cross-check `/healthz` watermark advanced past the UPDATE.
- **Playwright MCP not connected** — the browser tools error out → fall back to curl-ing the example's todos API route and PgPaw `/query` / `/q/{hash}/{version}` directly; record that the UI assertions were verified at the API layer, not visually.
- **Port already in use (3000 / 8080 / 5432)** — a stale dev server, pgpaw serve, or postgres container from a prior run is still bound → run the Cleanup Steps from the prior run (or `lsof -i :<port>` + kill) before re-launching; this is the FRESH WORKSPACE contract being violated.
- **`pnpm install` drags in unrelated a-zero workspace packages** — the `--ignore-workspace` flag was dropped (Step 9 or 12) → abort the install and re-run with the flag; a workspace-hijacked install breaks the standalone link.

## Cleanup

### Cleanup 1: Stop the Next dev server

**Action**: `pkill -f 'examples/nextjs-todos'`

**Observe**:
- The background Next process exits; port 3000 is freed (`lsof -i :3000` prints nothing)

**Awareness**:
- Match on the unique `examples/nextjs-todos` path so an unrelated `next dev` elsewhere is not killed. If port 3000 lingers in `CLOSE_WAIT`, force with `kill -9 <PID>`.

**On weirdness**: note-and-continue

### Cleanup 2: Stop the PgPaw serve process

**Action**: `pkill -f 'pgpaw serve'`

**Observe**:
- The background PgPaw process exits; port 8080 is freed (`lsof -i :8080` prints nothing)

**Awareness**:
- If multiple PgPaw instances are running, this pattern may match more than intended — verify with `lsof -i :8080` that only this run's listener went away.

**On weirdness**: note-and-continue

### Cleanup 3: Stop and remove the Postgres container

**Action**: `docker rm -f pgpaw-todos-pg`

**Observe**:
- Output prints the container name; `docker ps -a --filter name=pgpaw-todos-pg` is then empty

**Awareness**:
- A leftover container holds port 5432 and its volume holds stale `todos` rows — both break the next fresh run's FRESH WORKSPACE contract.

**On weirdness**: note-and-continue

### Cleanup 4: Unlink the global tanstack-db package

**Action**: `pnpm --dir examples/nextjs-todos unlink --global @pgpaw/tanstack-db`

**Observe**:
- The global link for `@pgpaw/tanstack-db` is removed from the example

**Awareness**:
- A leftover global link can shadow a future registry/`file:` install of the package. If this errors (e.g. "not linked"), it is harmless — note-and-continue rather than abort.

**On weirdness**: note-and-continue
