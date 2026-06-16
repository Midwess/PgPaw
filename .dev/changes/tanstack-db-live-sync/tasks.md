# Tasks: tanstack-db-live-sync

## Progress: [2/18]

## 1. PgPaw: transaction id on live deltas

- [x] 1.1 Thread `txn.xid` through `on_commit`; add `txid` to `encode` and `up_to_date`; emit `up-to-date{txid}` even when the diff is empty
- [x] 1.2 Rust unit test: `encode`/`up_to_date` carry the txid

## 2. PgPaw: reset on CDC lag

- [ ] 2.1 `reset_all` on `RecvError::Lagged` — emit `{"op":"reset"}` to all subs and clear them
- [ ] 2.2 Rust unit test: reset frame format

## 3. PgPaw: authenticated RLS live

- [ ] 3.1 `Subscription.principal`; `subscribe(..., principal)`; `on_commit` recompute via `query_json_as` when private
- [ ] 3.2 `http/query.rs`: remove the live `Forbidden`; `live_query(di, query, principal)`; private inline-rows first event
- [ ] 3.3 `cargo build` green (signature change + caller land together)

## 4. Library scaffold

- [ ] 4.1 `packages/tanstack-db/` — package.json (`@pgpaw/tanstack-db`, peer `@tanstack/db`), tsconfig, tsup + vitest config

## 5. Library: SSE reader + sync translation

- [ ] 5.1 `stream.ts` — SSE line reader over `fetch` + `ReadableStream`
- [ ] 5.2 `index.ts` — `pgpawCollectionOptions`: live connect, first-event branch (url/inline), deltas, reset → truncate+reload
- [ ] 5.3 Tests (mocked stream): initial load, delta apply, reset

## 6. Library: awaitTxId + writes

- [ ] 6.1 `txid.ts` — seen-store + `awaitTxId` (low-32-bit match, timeout)
- [ ] 6.2 Wrap `onInsert`/`onUpdate`/`onDelete` to await their returned txid
- [ ] 6.3 Tests: write resolves on txid seen; times out otherwise

## 7. Documentation

- [ ] 7.1 `packages/tanstack-db/README.md`
- [ ] 7.2 Update PgPaw `README.md` protocol section (txid, reset, private inline, RLS live)

## 8. Validation

- [ ] 8.1 `cargo build` + `cargo test`
- [ ] 8.2 `pnpm build` + test for the library (note if network-blocked)

---

## Notes

- User constraints: no code comments; commit + push to `main` after each step; no
  `Co-Authored-By` trailer.
- Commits group tightly-coupled tasks into one compiling unit to keep `main` green.
