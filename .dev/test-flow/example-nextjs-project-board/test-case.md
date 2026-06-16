---
id: example-nextjs-project-board
title: The nextjs-project-board example live-syncs a 3-table join, and a project rename propagates cross-table to every task badge
tier: live
component: examples
target: examples/nextjs-project-board
prerequisites:
  - "FRESH WORKSPACE REQUIRED — every live test MUST run against a freshly-created workspace (do NOT reuse a previously-tested one). Prior state on disk contaminates observations."
  - "No container named pgpaw-board-pg already running: `docker ps -a --filter name=pgpaw-board-pg --format '{{.Names}}'` returns empty"
  - "Port 5432 free: `lsof -iTCP:5432 -sTCP:LISTEN` returns empty"
  - "Port 8080 free: `lsof -iTCP:8080 -sTCP:LISTEN` returns empty"
  - "Port 3000 free: `lsof -iTCP:3000 -sTCP:LISTEN` returns empty"
  - "PgPaw release binary present: `test -x /Users/tiendang/Projects/a-zero/PgPaw/target/release/pgpaw`"
  - "Example schema present: `test -f /Users/tiendang/Projects/a-zero/PgPaw/examples/nextjs-project-board/schema.sql`"
  - "Docker daemon reachable: `docker info` exits 0"
  - "psql client available on PATH: `which psql`"
  - "Playwright MCP browser tools available (browser_navigate/browser_snapshot/browser_type/browser_click); if not, the API-curl fallback in Fail Modes applies"
expected_duration_secs: 900
tags: [examples, tanstack-db, nextjs, join, live-sync]
priority: high
created: 2026-06-16
author: senior-qa
---

## Objective

Verify that the nextjs-project-board example, driven by `@pgpaw/tanstack-db` against a live PgPaw + upstream Postgres, live-syncs a single collection built from a 3-table join, and that renaming a project propagates LIVE to every task row's project badge with no page reload.

## Preconditions

- This is a LIVE test: it requires a clean database and NO stale servers. Start from zero — no leftover `pgpaw-board-pg` container, no `pgpaw serve` process on 8080, no `pnpm dev` on 3000.
- Docker daemon is up: `docker info` exits 0.
- Ports 5432, 8080, 3000 are each free (probe individually; see prerequisites).
- Release binary exists and is executable: `test -x /Users/tiendang/Projects/a-zero/PgPaw/target/release/pgpaw`.
- Example files exist: `schema.sql` and `.env.example` under `examples/nextjs-project-board/`.

## Inputs

The join under test (one collection, three tables):

```sql
select t.id, t.title, t.completed, t.project_id,
       p.name as project, u.name as assignee
from todos t
join projects p on p.id = t.project_id
left join users u on u.id = t.assignee_id
```

Postgres connection string (used by psql Steps):

```text
postgresql://postgres:postgres@127.0.0.1:5432/myapp
```

Seed data created by `schema.sql`: projects p1/p2, users u1/u2, plus the `todos` table.

UI test values:

```text
New task title:      "Wire up live sync"
Renamed project:     old chip "Launch"  ->  new name "Launch v2"
```

## Steps

> **Notebook discipline**: Each Step is ONE action. The tester runs it, observes, judges, then moves to the next. No `&&`, `;`, multi-line shells, or for-loops inside a Step's Action. Polling = a Step the tester runs N times.

### Step 1: start-postgres

**Action**: `docker run -d --name pgpaw-board-pg -e POSTGRES_PASSWORD=postgres -e POSTGRES_DB=myapp -p 5432:5432 postgres:16 -c wal_level=logical`

**Observe**:
- A container id is printed (run succeeds).
- A new container `pgpaw-board-pg` now exists.

**Awareness**:
- Confirm `wal_level=logical` actually took effect later (Step 4 init / Step 8 serve depend on it). A container that starts but ignores the flag will fail logical replication downstream, not here.
- If `docker run` errors with a name conflict, the precondition "no pgpaw-board-pg" was violated — do not silently reuse it.

**On weirdness**: abort

### Step 2: wait-postgres-ready

**Action**: `docker exec pgpaw-board-pg pg_isready -U postgres`

**Observe**:
- Output ends with `accepting connections`.
- Exit status is 0.

**Awareness**:
- This is a polling Step: if it reports "no response" / "rejecting connections", re-run it (up to ~10 times, a couple seconds apart) before treating it as a failure. Do not chain a sleep into the Action.

**On weirdness**: retry once

### Step 3: apply-schema

**Action**: `psql postgresql://postgres:postgres@127.0.0.1:5432/myapp -f /Users/tiendang/Projects/a-zero/PgPaw/examples/nextjs-project-board/schema.sql`

**Observe**:
- CREATE TABLE / INSERT notices for projects, users, todos with no ERROR lines.
- The script completes and returns to the prompt.

**Awareness**:
- Verify all THREE tables were created (projects, users, todos), not just one — a partial schema would make the join return zero rows later.

**On weirdness**: abort

### Step 4: pgpaw-init

**Action**: `/Users/tiendang/Projects/a-zero/PgPaw/target/release/pgpaw init --pg-host 127.0.0.1 --pg-user postgres --pg-password postgres --pg-database myapp`

**Observe**:
- init reports success (publication/replication scaffolding created).
- Exit status 0.

**Awareness**:
- Watch for any warning about `wal_level` not being `logical` — if present, Step 1's flag did not apply and serve will later return 503.

**On weirdness**: abort

### Step 5: publish-three-tables

**Action**: `psql postgresql://postgres:postgres@127.0.0.1:5432/myapp -c "alter publication cache_server_pub add table projects, users, todos"`

**Observe**:
- `ALTER PUBLICATION` is reported (or an "already a member" notice).
- No ERROR line.

**Awareness**:
- An "already member" notice is FINE, not a failure. But confirm the END result is that all three of projects, users, todos are members — the join's cross-table propagation needs every table replicated.

**On weirdness**: note-and-continue

### Step 6: build-tanstack-db-install

**Action**: `pnpm install --ignore-workspace --dir /Users/tiendang/Projects/a-zero/PgPaw/packages/tanstack-db`

**Observe**:
- Install completes, dependencies resolved, no fatal errors.

**Awareness**:
- The `--ignore-workspace` flag is REQUIRED. Without it pnpm walks up to the parent `a-zero` workspace and hijacks resolution. If you see the parent workspace name in the install output, the flag was dropped — stop and re-run correctly.

**On weirdness**: abort

### Step 7: build-tanstack-db-compile

**Action**: `pnpm --dir /Users/tiendang/Projects/a-zero/PgPaw/packages/tanstack-db build`

**Observe**:
- tsup/build emits the compiled output with no TypeScript errors.
- A `dist/` (or configured output) is produced.

**Awareness**:
- Confirm the build actually wrote artifacts (non-empty output dir). A "build" that only typechecks would leave the link target empty and the example would fail to import.

**On weirdness**: abort

### Step 8: link-tanstack-db-global

**Action**: `pnpm --dir /Users/tiendang/Projects/a-zero/PgPaw/packages/tanstack-db link --global`

**Observe**:
- pnpm reports the package registered to the global store.

**Awareness**:
- The global link name must be `@pgpaw/tanstack-db` (the package.json name), not the folder name. Step 11 links by that exact name.

**On weirdness**: abort

### Step 9: example-install

**Action**: `pnpm install --ignore-workspace --dir /Users/tiendang/Projects/a-zero/PgPaw/examples/nextjs-project-board`

**Observe**:
- Install completes, no fatal resolution errors.

**Awareness**:
- `--ignore-workspace` is required here too (same parent-workspace hijack risk as Step 6).

**On weirdness**: abort

### Step 10: example-link-tanstack-db

**Action**: `pnpm --dir /Users/tiendang/Projects/a-zero/PgPaw/examples/nextjs-project-board link --global @pgpaw/tanstack-db`

**Observe**:
- pnpm reports `@pgpaw/tanstack-db` linked into the example's node_modules.

**Awareness**:
- After this, the example's `node_modules/@pgpaw/tanstack-db` should be a symlink pointing at the package. If it's a regular copy or missing, the import will resolve to nothing.

**On weirdness**: abort

### Step 11: copy-env

**Action**: `cp /Users/tiendang/Projects/a-zero/PgPaw/examples/nextjs-project-board/.env.example /Users/tiendang/Projects/a-zero/PgPaw/examples/nextjs-project-board/.env`

**Observe**:
- `.env` now exists in the example folder.

**Awareness**:
- Check the PgPaw URL inside `.env` points at `http://127.0.0.1:8080` (matching Step 12's serve port). A mismatched port here is a silent cause of an empty UI.

**On weirdness**: abort

### Step 12: start-pgpaw-serve

**Action**: `/Users/tiendang/Projects/a-zero/PgPaw/target/release/pgpaw serve --pg-host 127.0.0.1 --pg-user postgres --pg-password postgres --pg-database myapp --port 8080`

**Observe**:
- Long-running process; startup log indicates it bound to 8080 and connected to Postgres.
- Process stays up (this Step is backgrounded).

**Awareness**:
- This is a long-running Step — run it backgrounded; do NOT wait for it to exit. Watch the first lines for any 503/halt or "wal_level" complaint.

**On weirdness**: abort

### Step 13: check-pgpaw-health

**Action**: `curl -s http://127.0.0.1:8080/healthz`

**Observe**:
- HTTP 200 with a JSON/health payload that includes a watermark value.
- The watermark is present (replication is tracking a position), not null/empty.

**Awareness**:
- A 503 here means PgPaw is halted (typically wal_level not logical, or no REPLICA identity) — that is a hard stop for the live-sync behavior. This is a polling Step: retry a few times shortly after Step 12 starts.

**On weirdness**: retry once

### Step 14: start-example-dev

**Action**: `pnpm --dir /Users/tiendang/Projects/a-zero/PgPaw/examples/nextjs-project-board dev`

**Observe**:
- Next.js dev server compiles and reports listening on http://localhost:3000.
- No module-resolution error for `@pgpaw/tanstack-db` in the startup log.

**Awareness**:
- This is a long-running Step — background it. The most common startup failure is "Cannot find module @pgpaw/tanstack-db" (link broke); catch it in these first log lines, not later in the browser.

**On weirdness**: abort

### Step 15: open-board

**Action**: `browser_navigate http://localhost:3000`

**Observe**:
- The board page loads without a runtime error overlay.
- Initial layout is present (header / board area).

**Awareness**:
- If Playwright MCP is not connected, fall back to `curl -s http://localhost:3000` and the example's API route + PgPaw query endpoint (see Fail Modes) and note that visual checks are degraded.

**On weirdness**: note-and-continue

### Step 16: snapshot-initial-board

**Action**: `browser_snapshot`

**Observe**:
- Project chips labelled "Launch" and "Backlog" are visible.
- A project select control is present.

**Awareness**:
- The seed has projects p1/p2; confirm BOTH chips render. A single chip (or none) suggests the projects collection or the join did not load.

**On weirdness**: note-and-continue

### Step 17: choose-project

**Action**: `browser_click` the project select / "Launch" chip to set the active project for a new task.

**Observe**:
- The active project becomes "Launch" (selection reflected in the UI).

**Awareness**:
- Make sure the chosen project is the one you will rename later (Step 22). Keep the project identity consistent across add and rename steps.

**On weirdness**: retry once

### Step 18: type-task-title

**Action**: `browser_type` the value `Wire up live sync` into the new-task title input.

**Observe**:
- The input now holds the typed title.

**Awareness**:
- Confirm focus landed on the title field, not the project-rename field — typing into the wrong input would silently corrupt a later step.

**On weirdness**: retry once

### Step 19: submit-task

**Action**: `browser_click` the add/submit control for the new task.

**Observe**:
- A new task row appears in the board.
- The row carries the "Launch" project badge.

**Awareness**:
- Watch for an optimistic row that later disappears (would indicate the write to PgPaw failed and the optimistic update rolled back).

**On weirdness**: note-and-continue

### Step 20: verify-task-in-db

**Action**: `psql postgresql://postgres:postgres@127.0.0.1:5432/myapp -c "select id, title, project_id from todos"`

**Observe**:
- A row with title `Wire up live sync` exists.
- Its `id` is a uuid and `project_id` matches the "Launch" project.

**Awareness**:
- The `id` must be a real uuid (system-composed), not null/blank — confirms the insert round-tripped through PgPaw to Postgres rather than living only in client optimistic state.

**On weirdness**: abort

### Step 21: toggle-task-complete

**Action**: `browser_click` the complete/checkbox control on the `Wire up live sync` row.

**Observe**:
- The row's UI flips to a completed state.

**Awareness**:
- Note whether the flip is instant (optimistic) vs after a round-trip; either is acceptable, but a flip that reverts indicates a failed write.

**On weirdness**: note-and-continue

### Step 22: verify-toggle-in-db

**Action**: `psql postgresql://postgres:postgres@127.0.0.1:5432/myapp -c "select title, completed from todos where title = 'Wire up live sync'"`

**Observe**:
- The row shows `completed = t` (matching the UI state from Step 21).

**Awareness**:
- If UI shows completed but psql shows false, the toggle never reached Postgres — distinguish UI-only state from persisted state here.

**On weirdness**: note-and-continue

### Step 23: rename-project-open

**Action**: `browser_click` the "Launch" project chip to open its rename control.

**Observe**:
- A rename input/affordance appears for the "Launch" project.

**Awareness**:
- Ensure you opened rename on "Launch" (the project the Step 19 task belongs to), so the propagation in Step 26 is observable on an existing task row.

**On weirdness**: retry once

### Step 24: rename-project-type

**Action**: `browser_type` the value `Launch v2` into the project rename input.

**Observe**:
- The rename input holds `Launch v2`.

**Awareness**:
- Confirm you are editing the project name field, not creating a new project or editing a task title.

**On weirdness**: retry once

### Step 25: rename-project-submit

**Action**: `browser_click` the confirm/save control for the project rename.

**Observe**:
- The "Launch" chip label updates to "Launch v2".

**Awareness**:
- The chip is driven by the single-table `projects` collection — its update may show first, before the join-fed task badges. That ordering is fine; the headline check is Step 26.

**On weirdness**: note-and-continue

### Step 26: observe-cross-table-propagation

**Action**: `browser_snapshot`

**Observe**:
- EVERY task row that belonged to the renamed project now shows the badge "Launch v2" (including the Step 19 task) — live, with NO page reload.
- The change appears within a few seconds of Step 25.

**Awareness**:
- THIS IS THE HEADLINE BEHAVIOR — one collection over a 3-table join propagating a cross-table change. If the chip changed but task badges still read "Launch", the join is not live-resolving the projects side; that is a real failure, not a caveat.

**On weirdness**: note-and-continue

### Step 27: verify-rename-in-db

**Action**: `psql postgresql://postgres:postgres@127.0.0.1:5432/myapp -c "select name from projects"`

**Observe**:
- One project name is now `Launch v2`; the other is still "Backlog".

**Awareness**:
- This confirms the rename persisted upstream (not a UI-only edit). If psql still shows "Launch", the rename write never reached Postgres and the UI change in Step 25 was illusory.

**On weirdness**: note-and-continue

### Step 28: delete-task-via-ui

**Action**: `browser_click` the delete control on the `Wire up live sync` task row.

**Observe**:
- The task row disappears from the board.

**Awareness**:
- A delete done THROUGH the UI is the supported path and SHOULD remove the row. Contrast with the psql-side delete limitation noted in Expected Behavior / Fail Modes — do not conflate the two.

**On weirdness**: note-and-continue

## Expected Behavior

- PgPaw `/healthz` returns 200 with a non-empty watermark, indicating logical replication is connected and tracking a position.
- The board page renders both seeded project chips ("Launch", "Backlog") and a project-select control; no runtime error overlay.
- Adding a task under the chosen project produces a UI row carrying that project's badge, AND a corresponding `todos` row in psql with a uuid `id` and the correct `project_id`. (Exact row ordering / styling can vary; the persisted shape is what matters.)
- Toggling a task flips its completed state in the UI and the change is reflected as `completed = true` in psql for that row.
- HEADLINE: renaming a project propagates cross-table — the project chip relabels AND every task row carrying that project shows the NEW project name LIVE (no reload) within a few seconds. psql confirms the new name in `projects`. This is the "one collection over a 3-table join" payoff.
- Deleting a task THROUGH the UI removes the row from the board.
- The `project` and `assignee` join column names (system-composed by the select) are the aliases that feed the badges — exact-match those two names if inspecting raw payloads; everything else (labels, ordering, counts, wording) is behavioral.
- KNOWN JOIN CAVEAT (not a failure): this collection spans multiple tables with no single primary key, so a task DELETE performed DIRECTLY in psql (simulating another client) may NOT vanish from the UI until a reset. If the tester tries a psql-side delete, a lingering UI row is note-and-continue, NOT a fail. The supported delete path is through the UI (Step 28).

## Fail Modes

- **Example can't resolve `@pgpaw/tanstack-db`** (Step 14 "Cannot find module") — global link broke or `--ignore-workspace` was dropped on install — re-run Steps 6-10; or as a workaround switch the example's dependency to `file:../../packages/tanstack-db` and re-install with `--ignore-workspace`.
- **PgPaw returns 503 / halted** (Step 13) — `wal_level` not `logical`, or a table lacks REPLICA IDENTITY — verify `docker exec pgpaw-board-pg psql -U postgres -c "show wal_level"` reads `logical`; recreate the container with the `-c wal_level=logical` flag if not.
- **Join rows missing or rename doesn't propagate** (Steps 16 / 26) — not all three tables in the publication — re-check Step 5 result with `psql ... -c "select tablename from pg_publication_tables where pubname='cache_server_pub'"`; all of projects, users, todos must appear.
- **Playwright MCP not connected** (Steps 15-28) — browser tools unavailable — fall back to `curl -s http://localhost:3000` for render + the example's API route and PgPaw query endpoint to assert the join payload (look for `project`/`assignee` keys and the renamed value); note that visual UI judgments are degraded but the propagation can still be inferred from payloads + psql.
- **Ports in use** (Steps 1 / 12 / 14) — a stale container or server from a previous run — this violates the fresh-state precondition; stop the offending process / `docker rm -f` the old container and restart the affected Step.
- **Task badge stays "Launch" after rename while chip shows "Launch v2"** (Step 26) — join is not live-resolving the projects side (replication lag, missing projects table in publication, or collection not re-deriving) — confirm Step 27 shows the rename persisted, then check projects is published (Fail Mode above); this is a genuine failure of the headline behavior.

## Cleanup

> Single-action each. Run in order. Fresh-state for the next run depends on these completing.

- Stop the Next.js dev server (terminate the backgrounded Step 14 process).
- Stop the PgPaw serve process (terminate the backgrounded Step 12 process).
- `docker rm -f pgpaw-board-pg`
- `pnpm --dir /Users/tiendang/Projects/a-zero/PgPaw/examples/nextjs-project-board unlink --global @pgpaw/tanstack-db` (note-and-continue on error — link may already be gone)
