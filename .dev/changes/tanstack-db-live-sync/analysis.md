# Codebase Analysis: tanstack-db-live-sync

## Similar features in the codebase

- **Live delta path** (`src/live.rs`) — `LiveHub` already subscribes to the CDC
  bridge, re-runs each subscription's SQL on commit, diffs against the previous
  snapshot (`src/diff.rs`), and pushes SSE row deltas. This is exactly the
  mechanism the txid + RLS work extends; nothing new is invented.
- **Access-controlled execution** (`src/http/query.rs::private_response`,
  `src/rows.rs::query_json_as`) — the non-live private path already runs queries
  under the token role via `db.query_as(role, claims, sql, &[])`. RLS live reuses
  this exact call inside `on_commit`.
- **Auth extraction** (`src/auth.rs`) — `Principal { role, claims_json }` and the
  `AuthOutcome` `FromRequest` already run on the `query` handler. The live path
  receives the same `Principal`; no new auth code.

## Architecture layers relevant to this change

- **Transport**: actix-web 4, `HttpResponse::streaming` of an
  `UnboundedReceiverStream<String>` carrying `text/event-stream` frames
  (`http/query.rs::live_query`).
- **Fan-out**: `LiveHub` (`Arc<Mutex<HashMap<u64, Subscription>>>`), one tokio
  task draining `CdcBridge::subscribe()` (a `tokio::broadcast`).
- **Execution**: free fns in `src/rows.rs` (`query_json`, `query_json_as`).
- **CDC source**: `pglite::CommittedTransaction { xid: u32, commit_lsn, end_lsn,
  commit_ts, changes }` (`pglite-rs/crates/pglite/src/replica/mod.rs:94`).

## Key types (line-accurate)

- `live.rs:14` `struct Subscription { tables, pk, sql, sender, last }`
- `live.rs:22` `type LiveJob = (u64, String, Option<String>, HashMap<String,Value>)`
- `live.rs:54` `fn subscribe(&self, sql, tables, hash, version, snapshot_body) -> Receiver`
- `live.rs:93` `async fn on_commit(&self, txn: &CommittedTransaction)`
- `live.rs:142` `fn encode(delta: &Delta) -> String`
- `live.rs:151` `fn up_to_date() -> String`
- `http/query.rs:62` live `Forbidden` block for private (to remove)
- `http/query.rs:101` `async fn live_query(di, query)`
- `rows.rs:18` `query_json_as(db, role, claims, sql)`
- `auth.rs:11` `Principal { role: String, claims_json: String }`

## Dependencies

- Internal: `cdc::CdcBridge`, `diff::{diff, keyed_map, Delta}`, `rows`, `auth::Principal`.
- External (Rust): existing — `pglite-rs`, `serde_json`, `tokio`. No new crates.
- External (TS, new): `@tanstack/db` (peer). Dev: `typescript`, `vitest`, `tsup`.

## Conventions to follow (from CLAUDE.md / project.md)

- `?` everywhere; one unified `CacheError` (thiserror). Map new HTTP statuses only
  in `error_response`.
- Least New Definitions: attach to `LiveHub` / `Subscription`; do NOT add an
  `Inner` struct. `#[derive(Clone)]` with per-field `Arc`.
- **No inline code comments.** (Also an explicit user instruction for this change.)
- Locks hidden behind `&self`, brief lock scopes, interior mutability.
- Surgical changes — public live (the `/q` pointer path) stays untouched.

## OpenSpec integration notes

- Two delta domains: `realtime` (backend wire) and `tanstack-db-client` (library).
- No pre-existing specs in `.dev/specs/` to modify; these are additive.
