# PgPaw + TanStack DB — Next.js todos

A minimal Next.js (App Router) todo app that live-syncs from PgPaw with
[`@pgpaw/tanstack-db`](../../packages/tanstack-db) and optimistic writes confirmed
by transaction id.

- **Reads** stream live from PgPaw into a TanStack DB collection (`lib/todos.ts`).
- **Writes** go to Next.js route handlers (`app/api/todos/...`) that write to
  Postgres and return `pg_current_xact_id()` — the collection awaits that txid in
  the sync stream before clearing the optimistic update.
- PgPaw stays read-only; it never receives a write.

```
 browser  ──insert/update/delete──▶  /api/todos (Next route)  ──▶  Postgres
    ▲                                        │ returns { txid }
    │  live SSE (snapshot + deltas)          ▼
  PgPaw  ◀────────── logical replication ── Postgres
    │
 @pgpaw/tanstack-db  ──▶  TanStack DB collection  ──▶  useLiveQuery
```

## Prerequisites

- Postgres (13+), reachable, superuser for the one-time `pgpaw init`.
- [`pgpaw`](../../README.md#install) installed.

## Setup

1. **Create the table** in your app database:

   ```bash
   psql "$DATABASE_URL" -f schema.sql
   ```

2. **Prepare + run PgPaw** against that database:

   ```bash
   pgpaw init  --pg-host 127.0.0.1 --pg-user postgres --pg-database myapp
   pgpaw serve --pg-host 127.0.0.1 --pg-user postgres --pg-database myapp --port 8080
   ```

   Ensure `todos` is in the publication PgPaw reads (see `schema.sql`).

3. **Configure env**:

   ```bash
   cp .env.example .env
   # NEXT_PUBLIC_PGPAW_URL -> where PgPaw serves (browser-reachable)
   # DATABASE_URL          -> Postgres, used only by the API routes (server)
   ```

4. **Install + run**:

   ```bash
   pnpm install
   pnpm dev
   ```

   > Until `@pgpaw/tanstack-db` is published to npm, link it locally:
   > `cd ../../packages/tanstack-db && pnpm build && pnpm link --global`, then
   > `pnpm link --global @pgpaw/tanstack-db` here (or use a `file:` dependency).

Open http://localhost:3000, add a todo, then change the same row directly in
Postgres (`update todos set completed = true where ...`) and watch it sync.

## How the pieces fit

`lib/todos.ts` — the collection. `sql` is plain Postgres (joins allowed). The
write handlers POST/PATCH/DELETE to the API and return `{ txid }`:

```ts
export const todoCollection = createCollection(
  pgpawCollectionOptions<Todo>({
    url: process.env.NEXT_PUBLIC_PGPAW_URL!,
    sql: "select id, title, completed from todos",
    getKey: (todo) => todo.id,
    onInsert: async ({ transaction }) => txidOf(await fetch("/api/todos", { ... })),
    // onUpdate / onDelete similar
  }),
)
```

`app/page.tsx` — render + optimistic mutations:

```tsx
const { data: todos } = useLiveQuery((q) =>
  q.from({ todo: todoCollection }).select(({ todo }) => ({ ...todo fields }))
)
todoCollection.insert({ id: crypto.randomUUID(), title, completed: false })
todoCollection.update(todo.id, (draft) => { draft.completed = !draft.completed })
todoCollection.delete(todo.id)
```

`app/api/todos/[id]/route.ts` — the write + txid, in one statement so the id
matches what replication carries:

```sql
with upd as (update todos set ... where id = $1)
select pg_current_xact_id()::xid::text as txid
```

## Notes

- This example uses **client-generated `id`s** (`crypto.randomUUID()`), so the pk
  is a string and live deletes line up cleanly — the recommended shape for a
  single-table collection (see the package README's Limitations).
- **RLS**: this demo serves a public table (no token). For user-scoped data,
  configure PgPaw with a JWT key, enable RLS on the table, and pass the token:

  ```ts
  pgpawCollectionOptions({
    // ...
    headers: () => ({ authorization: `Bearer ${getToken()}` }),
  })
  ```

  PgPaw then streams the live deltas under the token's role.
