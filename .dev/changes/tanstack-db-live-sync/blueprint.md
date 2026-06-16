# Architecture Blueprint: tanstack-db-live-sync

## Design summary

Two surfaces. The backend gains protocol-neutral primitives (transaction id,
authenticated live, reset). The new npm package translates PgPaw's SSE into the
TanStack DB sync interface. PgPaw never references TanStack.

## Surface A — PgPaw (Rust)

### `src/live.rs`

`Subscription` gains a principal:

```rust
struct Subscription {
    tables: Vec<String>,
    pk: Option<String>,
    sql: String,
    sender: mpsc::UnboundedSender<String>,
    last: HashMap<String, Value>,
    principal: Option<Principal>,
}
```

`subscribe` gains `principal: Option<Principal>`. For a private subscription the
caller passes `snapshot_body` as the inline initial rows (already materialized
under the role); `keyed_map` seeds `last` identically for both paths.

`on_commit` recompute branches on the principal and threads the txid:

```rust
let fresh = match &principal {
    Some(p) => rows::query_json_as(&self.db, &p.role, &p.claims_json, &sql).await,
    None => rows::query_json(&self.db, &sql).await,
}.unwrap_or_else(|_| "[]".to_string());
...
for delta in &deltas { sub.sender.send(encode(delta, txid)); }
sub.sender.send(up_to_date(txid)); // emitted even when deltas is empty
```

`LiveJob` carries `xid` and `Option<Principal>`. `encode(delta, txid)` and
`up_to_date(txid)` add `"txid": <u32>`.

Reset on lag — in the `LiveHub::start` drain loop:

```rust
Err(RecvError::Lagged(_)) => worker.reset_all(),
```

`reset_all(&self)` sends `data: {"op":"reset"}\n\n` to every subscription and
clears the subs map (clients reconnect with a fresh snapshot).

### `src/http/query.rs`

- Delete the `if live { Forbidden }` block (private + live now allowed).
- `live_query(di, query, principal)`:
  - public → `materialize` → first event `{type:"snapshot", url, version}` (unchanged).
  - private → run `rows::query_json_as` once for the initial rows, send first event
    `{type:"snapshot", rows:<inline>, version}`, subscribe with `Some(principal)`.

`txid` value = `txn.xid` (the CDC `u32`). The client compares it against the low
32 bits of `pg_current_xact_id()`.

## Surface B — `packages/tanstack-db/`

```
packages/tanstack-db/
├── package.json        # name @pgpaw/tanstack-db, peer @tanstack/db
├── tsconfig.json
├── tsup.config.ts      # esm + cjs + d.ts
├── src/
│   ├── index.ts        # pgpawCollectionOptions
│   ├── stream.ts       # fetch + ReadableStream SSE line reader
│   └── txid.ts         # awaitTxId (low-32-bit), seenTxids store
└── test/
    ├── stream.test.ts
    └── collection.test.ts
```

`pgpawCollectionOptions(config)` returns a `collectionOptions` object:

```ts
export function pgpawCollectionOptions<T extends object>(config: {
  url: string
  sql: string
  getKey: (row: T) => string | number
  headers?: Record<string, string> | (() => Record<string, string>)
  onInsert?: InsertHandler<T>
  onUpdate?: UpdateHandler<T>
  onDelete?: DeleteHandler<T>
}) {
  const seen = new Set<number>()           // low-32-bit txids seen in sync
  return {
    getKey: config.getKey,
    sync: { sync: ({ begin, write, commit, markReady, truncate }) => {
      // POST {url}/query?live=true {sql}; read SSE
      // first event: url? -> GET {url}{url}; rows? -> use inline
      //   begin(); inserts -> write({type:'insert', value}); commit(); markReady()
      // delta insert/update/delete -> begin(); write(...); commit()
      // record headers/txid into `seen`
      // {op:'reset'} -> truncate(); restart from snapshot
      // return () => abort()
    }},
    onInsert: wrap(config.onInsert),       // await awaitTxId(returned txid)
    onUpdate: wrap(config.onUpdate),
    onDelete: wrap(config.onDelete),
    utils: { awaitTxId: (txid: number, timeoutMs?: number) => ... },
  }
}
```

`awaitTxId(txid)` resolves when `(txid & 0xffffffff)` is in `seen`; rejects on
timeout.

## Files to create / modify / review

| File | Action |
|------|--------|
| `src/live.rs` | modify — principal, txid, reset |
| `src/http/query.rs` | modify — lift 403, private inline first event |
| `packages/tanstack-db/*` | create — the library |
| `README.md` | modify — protocol docs |
| `src/cdc.rs` | review — confirms `CommittedTransaction` is forwarded (no change) |

## Implementation phases (ordered)

1. PgPaw: txid on deltas + up-to-date (compiles independently).
2. PgPaw: reset on lag.
3. PgPaw: RLS live (lift 403 + per-sub principal + inline first event).
4. Library scaffold.
5. Library: SSE reader + sync translation + reset.
6. Library: awaitTxId + write-handler wrapping.
7. Docs.

## Risks & mitigations

- **Always-green main**: phases 1–3 each compile on their own; the `subscribe`
  signature change (phase 3) and its only caller (`live_query`) land together.
- **No network for npm**: if `pnpm install` can't fetch `@tanstack/db`, the
  library code + tests are still authored and committed; build/test is noted as
  deferred in tasks Notes.
