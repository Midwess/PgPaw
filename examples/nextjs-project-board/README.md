# PgPaw — live join board

A Next.js board where **one collection live-syncs a 3-table join**
(`todos ⋈ projects ⋈ users`). Rename a project once and every task row updates
live — because PgPaw re-runs the join and streams the diff.

This is the thing a single-table-shape sync engine can't do in one collection:
it would need a separate sync per table plus a client-side join. PgPaw syncs the
joined SQL result directly.

```
select t.id, t.title, t.completed, t.project_id,
       p.name as project, u.name as assignee
from todos t
join projects p on p.id = t.project_id
left join users u on u.id = t.assignee_id
```

## What it shows

- **`boardCollection`** — the joined view above, one live collection.
- **`projectCollection`** — `select id, name from projects`, for the picker and rename.
- Toggle / add / delete a task → optimistic, confirmed by `txid`.
- **Rename a project** → its own collection updates *and* the `project` badge on
  every task in that project updates live, from a single write. That cross-table
  propagation is the payoff of syncing a join.

## Run

```bash
psql "$DATABASE_URL" -f schema.sql            # 3 tables + seed projects/users
pgpaw init  --pg-database myapp               # then publish all three tables
pgpaw serve --pg-database myapp --port 8080 --cors-origin http://localhost:3000
cp .env.example .env
pnpm install && pnpm dev
```

(Until `@pgpaw/tanstack-db` is on npm, link it locally — see the package README.)

Open http://localhost:3000, click a project chip to rename it, and watch every
task badge change at once.

## How writes work

The joined view is read-only; writes target the base tables and come back
through the join:

- `boardCollection.update(todoId, …)` → `PATCH /api/todos/:id` (writes `todos`)
- `projectCollection.update(projectId, …)` → `PATCH /api/projects/:id` (writes `projects`)

Each route writes in a transaction and returns `pg_current_xact_id()::xid` as the
`txid`; the collection awaits it before clearing the optimistic update. PgPaw
never receives a write.

## Note on joined collections

For a multi-table query PgPaw identifies rows by content (no single primary key),
so:

- **Inserts and updates sync live** (including cross-table changes like a project
  rename) — applied as upserts keyed by the task id.
- **Local optimistic deletes** work (removed immediately, confirmed by txid).
- A delete made **elsewhere** (another client, or directly in SQL) reconciles on
  the next `reset` rather than instantly. For instant cross-client deletes, use a
  single-table collection with a primary key (see `../nextjs-todos`).
