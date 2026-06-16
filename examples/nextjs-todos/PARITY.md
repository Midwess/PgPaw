# Parity with Electric's TanStack DB starter

Electric's canonical example is
[`electric-sql/electric/examples/tanstack-db-web-starter`](https://github.com/electric-sql/electric/tree/main/examples/tanstack-db-web-starter)
(TanStack Start + tRPC + Drizzle + Better Auth + an Electric shape proxy). This
note maps it, piece by piece, to the same app built on `@pgpaw/tanstack-db` —
to show the model is identical and where PgPaw does more or less.

## The write/sync pattern is the same

Electric's collection (`src/lib/collections.ts`):

```ts
export const todoCollection = createCollection(
  electricCollectionOptions<Todo>({
    id: "todos",
    shapeOptions: { url: "/api/todos", parser: { timestamptz: (s) => new Date(s) } },
    schema, getKey: (t) => t.id,
    onInsert: async ({ transaction }) => {
      const { modified } = transaction.mutations[0]
      const result = await trpc.todos.create.mutate(modified)
      return { txid: result.txid }
    },
  }),
)
```

PgPaw collection (`lib/todos.ts`):

```ts
export const todoCollection = createCollection(
  pgpawCollectionOptions<Todo>({
    url: process.env.NEXT_PUBLIC_PGPAW_URL!,
    sql: "select id, title, completed from todos",
    getKey: (todo) => todo.id,
    onInsert: async ({ transaction }) => {
      const todo = transaction.mutations[0].modified
      return txidOf(await fetch("/api/todos", { method: "POST", body: JSON.stringify(todo) }))
    },
  }),
)
```

Same `createCollection`, same `onInsert/onUpdate/onDelete → return { txid }`.

## Their txid helper is our txid SQL

Electric's `generateTxId(tx)` (run inside the mutation's transaction):

```sql
SELECT pg_current_xact_id()::xid::text AS txid
```

PgPaw's mutation route (`app/api/todos/[id]/route.ts`), same SQL, one statement:

```sql
with upd as (update todos set ... where id = $1)
select pg_current_xact_id()::xid::text as txid
```

Identical cast (`::xid::text` = the raw 32-bit xid the replication stream
carries). Electric wraps `generateTxId` + write in `db.transaction()`; PgPaw uses
a single CTE so the write and the txid read share one transaction — same result.

## File-by-file mapping

| Electric (`tanstack-db-web-starter`) | PgPaw (`examples/nextjs-todos`) | Note |
|---|---|---|
| `src/lib/collections.ts` (`electricCollectionOptions`) | `lib/todos.ts` (`pgpawCollectionOptions`) | same `createCollection` shape |
| `shapeOptions.url → /api/todos` (shape **proxy** route) | `url` → PgPaw directly + `headers` token | Electric needs a proxy route to inject auth and forward to `/v1/shape`; PgPaw is the endpoint and enforces RLS in-DB — **no proxy** |
| `src/lib/trpc/todos.ts` mutations → `{ item, txid }` | `app/api/todos/*` route handlers → `{ txid }` | same role, different transport (tRPC vs route handlers) |
| `generateTxId(tx)` → `pg_current_xact_id()::xid::text` | inline CTE → `pg_current_xact_id()::xid::text` | **identical SQL** |
| shape = `table` + `where` + `columns` (single table) | arbitrary read-only `SQL` incl. `JOIN` | PgPaw does more (below) |
| `useLiveQuery` (`@tanstack/react-db`) | `useLiveQuery` (`@tanstack/react-db`) | same hook — both are TanStack DB collections |

## Where PgPaw does more

- **One collection over a join.** `sql: "select t.*, u.name from todos t join users u …"`
  is a single PgPaw collection. The equivalent in Electric is multiple shapes
  (one per table) joined on the client — Electric shapes are single-table.
- **No shape-proxy route for auth.** Electric's `/api/todos` route validates the
  session and proxies to Electric. PgPaw verifies the JWT and runs the query under
  the token's role, so Postgres RLS filters the rows — point the collection at
  PgPaw with a bearer header, no proxy code.
- **`return { txid }` is optional.** Drop it for fire-and-forget (slight flicker);
  Electric's optimistic story leans harder on the txid round-trip.

## Where Electric is ahead (honest)

- **Resume.** Electric keeps a durable per-shape log → offset-precise catch-up after
  offline. PgPaw re-snapshots on a `reset` (the tradeoff for arbitrary SQL).
- **Maturity.** Multiple framework starters, React Native + SQLite offline,
  TanStack DB persistence, a larger ecosystem.

## Verdict

The integration contract a developer writes (collection options, write handlers
returning a txid, the `pg_current_xact_id()::xid::text` helper, `useLiveQuery`) is
the same on both. PgPaw trades Electric's offset-precise resume for plain-SQL
queries (joins in one collection) and drops the shape-proxy by enforcing RLS
in-database.

## Sources

- [electric/examples/tanstack-db-web-starter](https://github.com/electric-sql/electric/tree/main/examples/tanstack-db-web-starter)
- [Electric Collection — TanStack DB docs](https://tanstack.com/db/latest/docs/collections/electric-collection)
- [Electric apps get persistence and includes with TanStack DB 0.6](https://electric-sql.com/blog/2026/03/25/tanstack-db-0.6-app-ready-with-persistence-and-includes)
