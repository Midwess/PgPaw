# @pgpaw/tanstack-db

A native [TanStack DB](https://tanstack.com/db) collection for
[PgPaw](https://github.com/Midwess/PgPaw). It live-syncs **plain Postgres SQL**
(including joins) from PgPaw into a TanStack DB collection, enforces your
upstream Row-Level Security, and confirms optimistic writes by **transaction
id** — the same model as ElectricSQL, over PgPaw's SQL + SSE wire instead of
Electric's shape protocol.

PgPaw is read-only: writes go to **your** API, then replicate back through PgPaw
and confirm the optimistic update.

## Install

```bash
npm install @pgpaw/tanstack-db @tanstack/db
```

`@tanstack/db` is a peer dependency.

## Usage

```ts
import { createCollection } from "@tanstack/db"
import { pgpawCollectionOptions } from "@pgpaw/tanstack-db"

const todos = createCollection(
  pgpawCollectionOptions<Todo>({
    url: "https://pgpaw.example.com",
    sql: `select t.*, u.name as author
          from todos t join users u on u.id = t.user_id
          where t.org_id = 7`,
    getKey: (row) => row.id,

    // RLS / auth: send the same bearer token your PgPaw verifies
    headers: () => ({ authorization: `Bearer ${getToken()}` }),

    // Writes go to YOUR API; return the txid so the optimistic
    // update clears when the change syncs back through PgPaw.
    onInsert: async ({ transaction }) => {
      const { txid } = await api.todos.create(transaction.mutations[0].modified)
      return { txid }
    },
  }),
)
```

Your write API returns the transaction id from Postgres:

```sql
-- inside the same transaction as the write
select pg_current_xact_id() as txid;
```

The collection awaits that txid in the sync stream before dropping the optimistic
copy (matched on the low 32 bits, which is what PgPaw's CDC stream carries).

## How it works

- Opens `POST {url}/query?live=true` and reads PgPaw's SSE stream.
- First event is a snapshot: a `/q/{hash}/{version}` pointer for public queries
  (fetched once, CDN-cacheable) or inline `rows` for access-controlled (RLS)
  queries.
- Subsequent `insert` / `update` / `delete` events are applied to the collection;
  each carries the upstream `txid`.
- A `reset` event (PgPaw emitted on CDC lag) truncates the collection and
  re-loads the snapshot.

## API

`pgpawCollectionOptions<T>(config)` → options for `createCollection`.

| Field | Type | Notes |
|-------|------|-------|
| `url` | `string` | PgPaw base URL |
| `sql` | `string` | read-only `SELECT` PgPaw will serve |
| `getKey` | `(row: T) => string \| number` | row identity |
| `headers` | `Record<string,string>` \| `() => …` | per-request headers (auth) |
| `reconnectMs` | `number` | reconnect backoff (default `1000`) |
| `onInsert` / `onUpdate` / `onDelete` | handler returning `{ txid }` | optional write handlers |

`collection.utils.awaitTxId(txid, timeoutMs?)` — resolve when a txid has been
seen in the sync stream; rejects on timeout.

## Limitations

- **Live deletes need a single-table query with a primary key.** For multi-table
  (join) or no-pk queries PgPaw keys rows by a content hash that does not line up
  with `getKey`, so live *deletes* may not apply (inserts/updates still do). Use a
  single keyed table when you need precise live deletes.
- **Token lifetime.** The bearer token is verified when the stream opens; a
  long-lived stream keeps syncing under that token without re-checking `exp`.
  Reconnect to rotate credentials.
- **No offset-precise resume.** On CDC lag the client re-snapshots (`reset`)
  rather than gap-filling — the tradeoff for supporting arbitrary SQL.

## Develop

This package lives inside the PgPaw repo (a Rust-first submodule). Install and
test it standalone so the parent JS workspace is not picked up:

```bash
cd packages/tanstack-db
pnpm install --ignore-workspace
pnpm test
pnpm build
```

## License

MIT
