# Tasks: module-architecture-reshape

## Progress: [10/33]

Exit gate for EVERY phase: 4-combo feature matrix green (`--no-default-features --features read` · default · `--no-default-features --features az-wire` · `--all-features`), plus `cargo test --all-features --no-run` after phases 3 and 4. Moves are copy-paste + path/rename edits only — no body changes, no comment additions, no log-line edits.

## 1. protocol/ (leaf split of wire.rs)

- [x] 1.1 Create `src/protocol/subjects.rs` (the 3 `*_SUBJECT` consts) and `src/protocol/payload.rs` (all payload types + wire.rs test mod, schemars derives byte-identical)
- [x] 1.2 Create `src/protocol/mod.rs` (`pub mod payload; pub mod subjects;`); lib.rs: `pub mod wire` → `pub mod protocol` (read-gated)
- [x] 1.3 Repoint `src/az_wire.rs` wire imports to `crate::protocol::{payload, subjects}`; delete `src/wire.rs`; gate: `git grep 'crate::wire'` empty (also pulled tests/primary.rs wire-import fix forward from 7.1 to keep test compile green)

## 2. db/ (primary, shadow, setup)

- [x] 2.1 Create `src/db/shadow.rs` and `src/db/setup.rs` (verbatim; setup keeps TEMPORARY `crate::composition::UpstreamConfig` import — flipped in 4.6)
- [x] 2.2 Create `src/db/primary.rs` (verbatim; keep TEMPORARY `crate::composition::PrimarySource` import; preserve exact ungated/gated split: recovery helpers UNGATED, rest read-gated, unix/non-unix splits intact)
- [x] 2.3 Create `src/db/mod.rs` (child decls + gates); lib.rs: add `mod db;`, repoint `recover_primary`/`open_shadow`/`ShadowHandle` re-exports
- [x] 2.4 Repoint `src/composition.rs` calls (`crate::primary::*`, `crate::setup::*` → `crate::db::*`); delete `src/primary.rs`, `src/shadow.rs`, `src/setup.rs`; gate: read-only combo still exposes ungated recovery

## 3. capability/ (10 whole-file moves)

- [x] 3.1 Move classify, diff, rows, schema, cache, version → `capability/*` (sibling imports → `crate::capability::*`)
- [x] 3.2 Move cdc, live → `capability/*` (do NOT dedupe the duplicated `change_table` in live/version)
- [x] 3.3 Move auth → `capability/auth.rs` (`crate::operations::ReadOperations` → `crate::capability::read::ReadOperations`)
- [x] 3.4 Move operations → `capability/read.rs` — line-by-line gate preservation: `Replica` import/field/`HealthStatus`/`new`/`health` server-gated; `primary`'s inline `#[cfg(feature = "server")] replica: None,`; BOTH `is_private` complementary `#[cfg(server)]`/`#[cfg(not(server))]` arms
- [x] 3.5 Create `capability/mod.rs`; lib.rs: add `mod capability;` (read-gated), repoint `AuthConfig`/`ReadOperations`/`PreparedRead`/`HealthStatus` re-exports, remove 10 old mod lines
- [x] 3.6 Repoint remaining internal consumers: composition.rs, az_wire.rs, http/{server,query,health}.rs (+query tests), src/tests/mod.rs (4 imports)
- [x] 3.7 Delete the 10 old files; gate: stale-path grep empty + `cargo test --all-features --no-run` green

## 4. api/ + source/ (composition.rs split — one step, mutually referential)

- [ ] 4.1 Create `api/config.rs`: all config types with renames (`PgSource`, `EmbeddedPrimarySource`) and the 8-rule gate table from analysis §2 (`sslmode()` → pub(crate))
- [ ] 4.2 Create `api/runtime.rs`: `PgPaw` + Debug + all methods + `pub(crate) enum SourceShutdown` + composition tests (renamed call sites + explicit `use crate::api::config::{PgSource, EmbeddedPrimarySource};`)
- [ ] 4.3 Create `api/builder.rs`: `PgPawBuilder` + setters + `open()` calling `crate::source::build_read_core` (binding paths stay `crate::http`/`crate::az_wire` until Phase 5)
- [ ] 4.4 Create `source/replica.rs` + `source/primary.rs` (assembly bodies, byte-identical logs, imports rewritten) + `source/mod.rs` (dispatcher `build_read_core`)
- [ ] 4.5 Create `api/mod.rs`; lib.rs: add `mod api;` + `mod source;` (read-gated), repoint composition re-exports with renames
- [ ] 4.6 Flip Phase-2 temporary imports in `db/setup.rs` + `db/primary.rs` → `crate::api::config::{UpstreamConfig, EmbeddedPrimarySource}` (incl. `open_primary_db` param type rename)
- [ ] 4.7 Delete `src/composition.rs`; gate: `git grep 'crate::composition\|Self::build_read_core'` empty, matrix + test-compile green

## 5. binding/ (http 1:1 + az_wire)

- [ ] 5.1 Move `src/http/` → `binding/http/` 1:1 (`super::` route refs stay valid) and `src/az_wire.rs` → `binding/az_wire.rs`
- [ ] 5.2 Create `binding/mod.rs`; lib.rs: replace `mod http;`/`mod az_wire;` with `mod binding;`; flip `api/builder.rs` paths to `crate::binding::http::server::bind_at` / `crate::binding::az_wire::register_az_wire`
- [ ] 5.3 Delete `src/http/`, `src/az_wire.rs`; gate: `git grep 'crate::http::\|crate::az_wire::'` empty; all-features build verifies `#[handler]` macro unaffected

## 6. Crate-root reconcile

- [ ] 6.1 Finalize `lib.rs` (mod block + pub-use block per blueprint; `init()` delegates to `db::setup::prepare` unchanged)
- [ ] 6.2 Diff old vs new `pub use` symbol set — identical except `PgSource`/`EmbeddedPrimarySource` renames and `wire`→`protocol`
- [ ] 6.3 `src/main.rs`: import-list renames + 2 call sites (`PgSource::replica`, `PgSource::primary(EmbeddedPrimarySource{..})`); `mod tests` untouched; gate: `cargo test --all-features` PASSES

## 7. External consumers + README

- [ ] 7.1 `tests/primary.rs`: L1 imports; L11 `pgpaw::wire::{..}` → `pgpaw::protocol::{payload::LiveEvent, subjects::{LIVE_SUBJECT, READ_SUBJECT}}`; 11 rename sites (L36, 80, 112, 118, 213, 230, 252, 272-273, 294, 317)
- [ ] 7.2 `integration-tests/src/lib.rs` L145+L311, `integration-tests/tests/az_wire_replica.rs` L28, `bench/src/main.rs` L261: `pgpaw::Source::replica` → `pgpaw::PgSource::replica`
- [ ] 7.3 `README.md`: prose/snippet renames (~L268/275/289) + `target=` log examples (~L641-652 → `pgpaw::binding::http::{server,query}`, `pgpaw::capability::cache`) + one-line migration note for log consumers keyed on `target=`

## 8. Verification

- [ ] 8.1 Full 4-combo matrix + `cargo test --workspace --all-features` passes (incl. heavy primary/az-wire suites)
- [ ] 8.2 `cargo clippy --all-features -- -D warnings` + `cargo fmt --check` clean
- [ ] 8.3 Final greps: `pgpaw::wire|pgpaw::Source|::PrimarySource\b|pgpaw::http::|pgpaw::operations|crate::composition` return nothing; confirm no non-path diffs inside moved bodies (review gate)

---

## Notes

- STRONG PREREQUISITE: commit the current working tree (pre-existing work + unified-builder-api + review fixes) BEFORE applying — a whole-tree reshape on a dirty tree destroys rename detection and makes review impossible.
- Zero-shim incremental strategy: each phase moves a leaf and repoints its consumers in the same step; old and new paths never coexist. Phase 4 is deliberately larger (api+source mutually referential).
- R-new-1 park-and-flip: db/setup.rs + db/primary.rs temporarily import from `crate::composition` (Phase 2) until `api/config` exists (Phase 4.6).
- Log `target=` values change with `module_path!()` — unavoidable; `event=` names must stay byte-identical.
