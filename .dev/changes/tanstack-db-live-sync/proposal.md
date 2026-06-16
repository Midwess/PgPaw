# Proposal: TanStack DB live sync (`@pgpaw/tanstack-db`)

**Status**: approved

## Summary

Expose the upstream Postgres transaction id on PgPaw's live SSE stream and allow
authenticated live streaming of access-controlled (RLS) queries, then ship
`@pgpaw/tanstack-db` — a native TanStack DB collection that live-syncs full-SQL
queries with optimistic writes confirmed via transaction id.

## Motivation

Apps on [TanStack DB](https://tanstack.com/db) want live, optimistic-write
collections backed by PgPaw — including user-scoped (RLS) data, which is most
real collections. PgPaw cannot serve them today:

- **No write reconciliation.** Live deltas carry no transaction id, so a client
  has no way to know when its optimistic write has come back through sync. The
  upstream id already exists on the CDC record (`CommittedTransaction.xid`, a
  `u32`); it is simply never emitted.
- **RLS queries can't stream.** Live mode is hard-blocked for access-controlled
  queries (`http/query.rs:62` → `403`). A client library cannot bypass a server
  that refuses to stream.

This is the same architecture as ElectricSQL — a read-only sync engine, writes
to your own API, optimistic state reconciled by transaction id — but over
PgPaw's plain-SQL wire instead of Electric's shape protocol. The backend stays
protocol-neutral; only the new npm package knows TanStack DB exists.

## Scope

### In Scope

**PgPaw (Rust) — protocol-neutral, no TanStack reference:**
- Emit `txid` (low 32 bits of the upstream `xid`) on live `insert`/`update`/`delete`
  and `up-to-date` events. Emit `up-to-date` carrying `txid` even when the diff is
  empty, so a write outside the result set (or a no-op update) still resolves.
- Authenticated RLS live: lift the `403`; `LiveHub` holds an `Option<Principal>`
  per subscription and recomputes deltas under the role via `query_json_as`.
- Private live: first event carries inline `rows` (no cacheable `/q` pointer
  exists for private queries).
- `reset` control event when the CDC feed lags (`RecvError::Lagged`), so a client
  re-syncs instead of silently diverging.

**New library `@pgpaw/tanstack-db` (`packages/tanstack-db/`):**
- `pgpawCollectionOptions({ url, sql, getKey, headers?, onInsert/onUpdate/onDelete? })`.
- `fetch` + `ReadableStream` SSE reader → `begin`/`write`/`commit`/`markReady`.
- First event branch: `url?` → `GET /q/...`; `rows?` → use inline.
- `collection.utils.awaitTxId` (low-32-bit compare); write handlers wrap to await
  their returned txid.
- `reset` → `truncate()` + reload.

### Out of Scope

- Electric wire-protocol compatibility (`offset`/`handle`/`must-refetch`/`409`).
  The official `@tanstack/electric-db-collection` will NOT plug into PgPaw.
- Durable per-shape log / offset-precise resume. PgPaw re-snapshots on reset.
- Any mutation endpoint in PgPaw. It stays read-only; writes hit the user's API.
- Other framework adapters (Solid / Svelte / vanilla).
- Changes to the public live protocol (keeps the `/q` snapshot pointer).

## Affected Areas

| Area | Impact |
|------|--------|
| `src/live.rs` | `Subscription` gains `principal`; `subscribe` gains `principal` param; `on_commit` recomputes via `query_json_as` when private and threads `txn.xid`; `encode`/`up_to_date` gain `txid`; lag → `reset` |
| `src/http/query.rs` | Remove live `Forbidden` for private; `live_query` takes `Principal`; private first event = inline rows |
| `packages/tanstack-db/` | New npm package (the collection adapter) |
| `README.md` | Document `txid`, `reset`, private inline first event, RLS live |

## Dependencies

- `pglite-rs` already exposes `query_as(role, claims, sql, params)` and
  `CommittedTransaction.xid` — no pglite change required.
- Library: `@tanstack/db` (peer dependency).

## Risks

| Risk | Mitigation |
|------|------------|
| Per-subscriber RLS recompute on every commit is O(subscribers × SQL) | Already the existing model (per-sub re-run + diff); RLS only swaps the exec fn. Document as a known cost. |
| `awaitTxId` hangs if the confirming event never arrives | Emit `up-to-date{txid}` for any commit touching the subscription's tables even with an empty diff; client also resolves on a matching delta. |
| 64-bit `pg_current_xact_id()` vs 32-bit CDC `xid` mismatch | Match on the low 32 bits in `awaitTxId`. Documented in the client. |
| CDC lag silently diverges a client | `reset` event → client truncates and re-snapshots. |
| Pushing each step to `main` could break the build | Group tightly-coupled tasks into one compiling commit; `cargo build`/package build must pass before each push. |
