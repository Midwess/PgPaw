# Codebase Analysis: module-architecture-reshape

Generated: 2026-07-16
Scope: Pure file/module reshape of `src/` into `api/`, `source/`, `capability/`, `binding/`, `db/`, `protocol/` — zero behavior change. Renames `Source`→`PgSource`, `PrimarySource`→`EmbeddedPrimarySource`. Flat crate-root re-exports preserved.

Note on repo state: the working tree has substantial uncommitted changes from the just-finished `unified-builder-api` change. This analysis is against current working-tree content — the correct starting point for the reshape.

## 1. Item-by-item move map

### `src/lib.rs` → stays crate root; content replaced

- All `mod` declarations → new tree: `mod api; mod source; mod capability; mod binding; mod db; mod protocol; mod error;` (+ gates, see §2).
- `#[cfg(all(test, feature = "server"))] mod tests;` stays; `src/tests/mod.rs` content unchanged except 4 import lines (`crate::classify::` → `crate::capability::classify::` etc.).
- Flat re-exports (all preserved at crate root, sources updated):
  - `AuthConfig` ← `capability::auth` (see Ambiguity #1)
  - `UnbConfig`, `HttpConfig`, `UpstreamConfig`, `CacheConfig`, `ReplicaSource`, `EmbeddedPrimarySource` (renamed), `PgSource` (renamed) ← `api::config`
  - `PgPaw` ← `api::runtime`; `PgPawBuilder` ← `api::builder`
  - `CacheError`, `LifecycleErrorKind` ← `error` (unchanged)
  - `HealthStatus`, `PreparedRead`, `ReadOperations` ← `capability::read`
  - `recover_primary` ← `db::primary`
  - `open_shadow`, `ShadowHandle` ← `db::shadow`
- `pub async fn init(...)` (`#[cfg(feature = "server")]`) stays a crate-root free fn delegating to `db::setup::prepare` (Ambiguity #3 resolved).

### `src/composition.rs` → split into `api/config.rs`, `api/builder.rs`, `api/runtime.rs`, `source/{mod,replica,primary}.rs`

| Item | Target | Notes |
|---|---|---|
| `UpstreamConfig` + `Default` + `fn sslmode` | `api/config.rs` | `sslmode()` used by `source::replica` → becomes `pub(crate)` |
| `ReplicaSource` + `Default` | `api/config.rs` | struct = config; assembly logic moves to `source/replica.rs` |
| `PrimarySource` + `Default` + `fn embedded` | `api/config.rs`, **renamed `EmbeddedPrimarySource`** | |
| `enum Source` + `fn replica`/`fn primary` | `api/config.rs`, **renamed `PgSource`** | variant names `Replica`/`Primary` unchanged |
| `CacheConfig` + `Default` | `api/config.rs` | |
| `HttpConfig` | `api/config.rs` | server-gated |
| `UnbConfig` (private fields) | `api/config.rs` | unb-gated |
| `PgPawBuilder` struct + setter methods + `open()` | `api/builder.rs` | `open()` calls `crate::source::build_read_core`, `crate::binding::http::server::bind_at`, `crate::binding::unb::register_unb` |
| `build_read_core` dispatcher | `source/mod.rs` as `pub(crate) async fn build_read_core(source, cache, auth)` free fn (Ambiguity #4 — provisional) | dispatches once on `PgSource` |
| `build_replica_core` body | `source/replica.rs` (`#[cfg(feature = "server")]`, `pub(crate)` fn) | logs preserved byte-identical |
| `build_primary_core` body | `source/primary.rs` (ungated, `pub(crate)` fn) | |
| `enum SourceShutdown` | `api/runtime.rs` (user's tree annotation, Ambiguity #5 settled) | |
| `PgPaw` struct + `Debug` + all methods (`builder`, `primary_dsn`, `live_subscription_count`, `wait`, `shutdown`, `abort_open`) | `api/runtime.rs` | |
| composition tests (`open_requires_a_source`, `primary_source_opens_...`, `http_binding_over_primary_serves_health`, `http_bind_conflict_...` + `ensure_runtime_dir`/`http_get`) | `api/runtime.rs` tests | bodies need `PgSource::primary(EmbeddedPrimarySource::embedded(..))` renames; tests need explicit `use crate::api::config::{PgSource, EmbeddedPrimarySource};` since `super::*` won't pull them |

### `src/auth.rs` → `capability/auth.rs` (whole file)

`Principal`, `AuthConfig` (+ fluent ctors + `into_verifier`), `Verifier` (+ `build`/`verify`), `fn pinned`, `AuthOutcome` + `FromRequest` impl + `fn authenticate` (all three server-gated), test mod — all co-located. `authenticate`'s `crate::operations::ReadOperations` → `crate::capability::read::ReadOperations`.
**Ambiguity #1 resolved: `AuthConfig` stays in `capability/auth.rs`** — it carries behavior (`into_verifier`) coupled to `Verifier`; CONTEXT.md lists auth as a capability; user's tree annotation had `AuthConfig?` with a question mark. Crate-root flat re-export keeps the public path identical either way.

### `src/operations.rs` → `capability/read.rs` (whole file)

`SecurityCache` type alias, `ReadOperations`, `PreparedRead`, `HealthStatus` (server-gated), full impl (`new` server-gated, `primary` pub(crate), `health` server-gated, `live_subscription_count` pub(crate), `authenticate`, `prepare`, `execute_private`, `execute_public`, `materialize`, `cursor`, `materialize_version`, `subscribe`, `is_private`, `classify_security`), `merge_verdicts`, `map_db_denial`, test mod.

### `src/cdc.rs` → `capability/cdc.rs`; `src/cache.rs` → `capability/cache.rs`; `src/live.rs` → `capability/live.rs`; `src/version.rs` → `capability/version.rs`; `src/rows.rs` → `capability/rows.rs`; `src/schema.rs` → `capability/schema.rs`; `src/classify.rs` → `capability/classify.rs`; `src/diff.rs` → `capability/diff.rs`

Whole-file moves, all items co-located, test mods travel with their file. Note: `fn change_table` is duplicated verbatim in `live.rs` and `version.rs` (both private) — pre-existing duplication, **do NOT deduplicate** (Surgical Changes). Each copy moves with its file.

### `src/primary.rs` → `db/primary.rs` (whole file, no internal split)

`open_primary_db` (read-gated; param renamed `&EmbeddedPrimarySource`, import `crate::api::config::EmbeddedPrimarySource`), `recover_primary` (public, UNGATED), `primary_start_error` (read-gated), `recover_primary_inner` unix/non-unix (ungated), `primary_is_busy` (unix+read / non-unix+read), `process_is_alive`/`remove_pid_file`/`kill` extern (unix, ungated), `PrimaryObserver` struct+impl (read-gated, pub(crate)) — **Ambiguity #6 resolved to `db/primary.rs` per user's tree annotation "observer primitives"**. Test mod (`startup_errors_have_stable_categories`, read-gated) co-located. The ungated/gated split within this file must be preserved exactly — recovery must stay available without `read`.

### `src/shadow.rs` → `db/shadow.rs`; `src/setup.rs` → `db/setup.rs`

Whole-file moves. setup.rs `use crate::composition::UpstreamConfig` → `crate::api::config::UpstreamConfig`.

### `src/wire.rs` → split: `protocol/subjects.rs` (the 3 `*_SUBJECT` consts) + `protocol/payload.rs` (everything else: `ReadRequest`, `ReadResponse`, `CursorRequest`, `CursorResponse`, `LiveRequest`, `LiveEvent`, `WireError`, test mod)

Unambiguous split — no item straddles. **Risk R1**: `pub mod wire` is today a public submodule path (`pgpaw::wire::{..}`, used by `tests/primary.rs:11`). Target tree replaces it with `pgpaw::protocol::{payload, subjects}` — intentional public rename, in-scope; update the one first-party import (no compat shim).

### `src/unb.rs` → `binding/unb.rs` (whole file)

`register_unb` (pub(crate)), 3 `#[handler]` fns, private helpers (`parse_rows`, `decode_event`, `string`, `number`, `handler_error`), test mod. Imports: `crate::operations::ReadOperations` → `crate::capability::read::ReadOperations`; `crate::wire::{..}` → `crate::protocol::{payload::.., subjects::..}`.

### `src/http/{mod,server,query,health}.rs` → `binding/http/{mod,server,query,health}.rs` (1:1)

Sibling relationship preserved — `super::health`/`super::query` route refs in `server.rs` stay valid unchanged. `error_response`/`error_status` used only within `query.rs` (unb has its own parallel `handler_error`) — stay local. Imports update to `crate::capability::{read, auth, classify}`.

### `src/error.rs` → stays (crate root). `src/main.rs` → stays (bin crate root, CLI adapter)

main.rs edits are rename-only: import list (`PrimarySource`→`EmbeddedPrimarySource`, `Source`→`PgSource`), `Source::replica(..)` → `PgSource::replica(..)` (line ~237), `Source::primary(PrimarySource{..})` → `PgSource::primary(EmbeddedPrimarySource{..})` (line ~261). `mod tests` stays put — the `--exact tests::shutdown_signal_helper` subprocess path is unaffected (Risk R6 cleared).

## 2. Feature-gate map

| New module decl | Gate |
|---|---|
| `mod api;` (lib.rs) | `#[cfg(feature = "read")]` — but item-level gates inside per table below |
| `mod source;` | `#[cfg(feature = "read")]`; inside `source/mod.rs`: `#[cfg(feature = "server")] mod replica;` (whole file server-gated), `mod primary;` ungated |
| `mod capability;` | `#[cfg(feature = "read")]`; inside: each `mod X;` line individually gated to match the current explicit per-item style (all read; fine since parent gated — keep individual gates for style parity) |
| `mod binding;` | ungated file; inside: `#[cfg(feature = "server")] mod http;`, `#[cfg(feature = "unb")] mod unb;` |
| `mod db;` | ungated; inside: `mod primary;` ungated, `mod shadow;` ungated, `#[cfg(feature = "server")] mod setup;` |
| `mod protocol;` | `#[cfg(feature = "read")]` (matches current `pub mod wire` gate); `pub mod payload; pub mod subjects;` inside |
| `mod error;` | ungated (unchanged) |

### Highest-risk gate redistribution: composition.rs split

| Item | Gate to preserve at destination |
|---|---|
| `UpstreamConfig`, `ReplicaSource`, `HttpConfig`, `PgSource::Replica` variant, `PgSource::replica()` | `#[cfg(feature = "server")]` |
| `EmbeddedPrimarySource`, `PgSource::Primary` variant, `PgSource::primary()`, `CacheConfig`, `build_primary_core` | ungated |
| `UnbConfig`, `PgPawBuilder::unb`, `PgPaw.unb` field | `#[cfg(feature = "unb")]` |
| `PgPawBuilder.http` field, `::http` method, open()'s http block, `PgPaw.http_handle`/`http_task`, `SourceShutdown::Replica` variant, shutdown()'s Replica arm | `#[cfg(feature = "server")]` |
| `build_read_core`'s `Replica` match arm | `#[cfg(feature = "server")]` on the arm |
| `abort_open` | `#[cfg(any(feature = "server", feature = "unb"))]` |
| `open()`'s `let mut pgpaw` + `shutdown(mut self)` | `#[cfg_attr(not(any(feature = "server", feature = "unb")), allow(unused_mut))]` |
| composition http tests (`http_get`, both http tests) | `#[cfg(feature = "server")]` |

### operations.rs → capability/read.rs (second-densest)

`use pglite::Replica`, `replica` field, `HealthStatus`, `::new`, `::health` → server-gated; `::primary`'s inline `#[cfg(feature = "server")] replica: None,` field-init preserved exactly; `is_private`'s complementary `#[cfg(feature = "server")]` / `#[cfg(not(feature = "server"))]` version branches — **highest single-item risk in the reshape**: a dropped or flipped arm silently changes behavior or breaks a combo.

### primary.rs → db/primary.rs

Ungated: `recover_primary`, `recover_primary_inner` (both), `process_is_alive`, `remove_pid_file`, `kill`. Read-gated: everything else. Preserve exactly — recovery tests run without `read`.

## 3. Visibility/dependency graph

All cross-file calls use `pub(crate)` items (crate-wide — moves mechanically safe). No bare-private fn is called cross-file. `super::` relative refs only in `http/server.rs` routes — preserved by the 1:1 `binding/http/` move.

Path rewrites (complete list): see per-file notes in §1 plus:
- `composition` → uses of `crate::{schema, setup, primary, http, unb, auth, cache, cdc, live, operations, version}` → `crate::{capability::schema, db::setup, db::primary, binding::http, binding::unb, capability::auth, capability::cache, capability::cdc, capability::live, capability::read, capability::version}`
- `operations`/`live`/`http/*`/`unb` internal `use crate::X` → `crate::capability::X` / `crate::protocol::X` (keep absolute `crate::` paths, not `super::`, for mechanical consistency)
- `http/query.rs` (+ its tests): `crate::operations::map_db_denial` → `crate::capability::read::map_db_denial`

Post-reshape dependency direction:
```
api        → source, capability, binding, db, error
source     → api::config, capability, db, error
binding    → capability, protocol, error
capability → error
db         → capability (ONE edge: PrimaryObserver::start takes capability::cdc::CdcBridge), api::config (EmbeddedPrimarySource), error
protocol   → (leaf)
error      → (leaf)
```
**R2**: the `db → capability::cdc` edge is pre-existing and accepted — do NOT invert it (behavior refactor, out of scope).

## 4. Consumer inventory (exact edits)

| File | Edits |
|---|---|
| `tests/primary.rs` | line 1 import renames; line 11 `pgpaw::wire::{..}` → `pgpaw::protocol::{payload::LiveEvent, subjects::{LIVE_SUBJECT, READ_SUBJECT}}`; 11 token sites `Source::primary`/`PrimarySource` → renamed (lines 36, 80, 112, 118, 213, 230, 252, 272-273, 294, 317) |
| `tests/topology_benchmark.rs` | none (raw unb + subject string literal only) |
| `integration-tests/src/lib.rs` | lines 145, 311: `pgpaw::Source::replica` → `pgpaw::PgSource::replica` (only 2 sites) |
| `integration-tests/tests/unb_replica.rs` | line 28: same rename (1 site) |
| other `integration-tests/tests/*.rs` | none (harness-only consumers) |
| `bench/src/main.rs` | line 261: same rename (1 site) |
| `src/main.rs` | import list + 2 call sites (§1) |
| `README.md` | lines ~268/275/289 prose+snippet renames; lines ~641-652 `target=pgpaw::http::server` etc. log examples → new module paths (see R3) |
| Cargo.tomls | none |

## 5. Test-module placement

Every test module maps 1:1 to its file's destination (see §1). Special cases:
- composition tests → `api/runtime.rs`, need explicit `use crate::api::config::{PgSource, EmbeddedPrimarySource};` (not covered by `super::*`).
- `src/tests/mod.rs` stays; 4 import lines update.
- `main.rs` tests stay; `--exact tests::shutdown_signal_helper` path unaffected.
- wire.rs tests → `protocol/payload.rs` (both exercise payload types).

## 6. Conventions and risks

Conventions: logfmt `event=` names byte-identical (pure moves preserve automatically — do not "clean up" log lines); no inline comments (also no "// moved from X" comments); no new `.unwrap()` where `?` existed; no new wrapper structs invented to host moved free fns.

| Risk | Impact | Mitigation |
|---|---|---|
| R1: `pgpaw::wire` public path replaced by `pgpaw::protocol::{payload,subjects}` | breaks `tests/primary.rs:11` | intentional in-scope rename; update the one first-party import; no compat shim |
| R2: `db → capability::cdc` edge | layering surprise only | pre-existing; accepted; do not invert |
| R3: log `target=` values change with `module_path!()` (e.g. `pgpaw::operations` → `pgpaw::capability::read`) | downstream log parsing keyed on `target=` breaks silently; `event=` unaffected | unavoidable in a module move; do NOT add explicit `target:` params (scope creep); update README log examples; note in change docs |
| R4: cfg-gate omission during composition.rs / operations.rs splits | silent behavior change or combo compile failure | build all 4 feature combos after every phase (`--no-default-features --features read`, default `server`, `unb`, `--all-features`) — precedent: commit b3f3038 |
| R5: missed flat re-export | hard compile failure downstream | diff old vs new `pub use` list before completion |
| R6: `main.rs` `--exact tests::shutdown_signal_helper` | none — main.rs untouched | confirmed safe |
| R7: AuthConfig / PrimaryObserver placement ambiguity | churn if re-decided later | resolved: `capability/auth.rs` and `db/primary.rs` (§1) — treat as authoritative |

## Ambiguities (resolved)

1. `AuthConfig` → `capability/auth.rs` (behavior-coupled to `Verifier`; CONTEXT.md lists auth as capability; flat re-export hides the location).
2. `ReplicaSource` struct in `api/config.rs`, assembly in `source/replica.rs` — config/logic split by design, not a conflict.
3. `init()` → stays crate-root free fn in `lib.rs`, delegates to `db::setup::prepare`.
4. `build_read_core` → `source/mod.rs` free fn (provisional — the one genuine judgment call; architect to confirm).
5. `SourceShutdown` → `api/runtime.rs` (user's tree annotation).
6. `PrimaryObserver` → `db/primary.rs` (user's tree annotation), accepting the R2 edge.

## Recommended phase ordering

1. `protocol/` (leaf) → 2. `db/` (shadow, setup, primary — primary after capability/cdc exists if strict, but pub(crate) is crate-wide so order is compile-safe either way) → 3. `capability/` (classify, diff, cache, version, rows, schema → cdc → auth → live → read last) → 4. `api/` (config + builder + runtime in one phase to avoid half-renamed states) → 5. `source/` (replica, primary, mod dispatcher) → 6. `binding/` (http 1:1, unb) → 7. crate-root `lib.rs` re-export wiring + `init()` → 8. `src/tests/mod.rs` imports → 9. external consumers + README. Build the 4-combo feature matrix after each phase.

## Confidence Assessment

- Pattern confidence: 95 — every file read in full; move map cross-checked against `git grep`.
- Architecture understanding: 92 — dependency graph + gate matrix derived from source; the `db → capability` edge is a known pre-existing wrinkle.
- Ambiguity resolutions: 80 — five of six anchored in CONTEXT.md/user annotations; only #4 (`build_read_core` home) is a genuine judgment call, provisional.
