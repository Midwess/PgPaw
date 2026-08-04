# Architecture Blueprint: module-architecture-reshape

Generated: 2026-07-16
Based on: analysis.md, CONTEXT.md

## Design Summary

A pure file/module reshape of PgPaw's flat `src/` into six responsibility-scoped modules (`api/`, `source/`, `capability/`, `binding/`, `db/`, `protocol/`), plus two public renames (`Source`→`PgSource`, `PrimarySource`→`EmbeddedPrimarySource`) and one public path rename (`pgpaw::wire`→`pgpaw::protocol::{payload,subjects}`). Zero behavior change: every moved item keeps its exact `#[cfg]` gate, log strings, and body byte-for-byte; the public surface is preserved via flat crate-root re-exports (except the intentional renames).

## Design Decisions

| Decision | Chosen | Rationale |
|----------|--------|-----------|
| `build_read_core` home (Ambiguity #4) | `pub(crate) async fn` free fn in `source/mod.rs` | It's a pure dispatcher over `PgSource` delegating to the two assembly bodies CONTEXT.md assigns to `source/`. Keeping it on `PgPawBuilder` drags assembly imports across the `api → source` boundary; attaching to `PgSource` would collapse the config/assembly split by forcing `api/config.rs` to import `PGlite`/`ReadOperations`/`SourceShutdown`. The two `build_*_core` bodies are already `pub(crate)` free fns in `source/` (they return a 4-tuple; CLAUDE.md says prefer tuples, don't invent wrapper structs to host moved fns) — the dispatcher belongs beside them. Struct-First yields here to Least New Definitions + Strict Placement. |
| Migration strategy | Incremental, bottom-up, ZERO shims | Big-bang compiles only at the end — a dropped cfg gate then means bisecting ~20 moved files. Incremental runs the 4-combo matrix after each layer, localizing gate regressions. No shims needed: tree is uncommitted (no external mid-flight consumers) and each phase moves a leaf + repoints its consumers in the same step, so old and new paths never coexist. |
| Phase 4+5 merge | `api/` and `source/` land in one step | Mutually referential: `api/builder.rs::open` calls `crate::source::build_read_core`; the dispatcher's signature names `api::runtime::SourceShutdown`. Splitting would need a temporary shim — rejected. |
| `AuthConfig` home | `capability/auth.rs` | Behavior-coupled to `Verifier` via `into_verifier`; flat re-export keeps `pgpaw::AuthConfig` identical. |
| `wire` → `protocol` visibility | `pub mod protocol { pub mod payload; pub mod subjects; }` | Intentional in-scope public rename; only first-party consumer (`tests/primary.rs:11`) updated; no compat shim. |
| `PrimaryObserver` home | `db/primary.rs` | User tree annotation "observer primitives"; accepts pre-existing `db → capability::cdc` edge (R2). |
| `SourceShutdown` home | `api/runtime.rs`, visibility widened to `pub(crate)` | User tree annotation; it is `PgPaw`'s shutdown state; `pub(crate)` needed so the `source/` dispatcher can name it. |

## mod.rs Contents & Visibility

Only `protocol` is a public module; everything else internal, exposed via flat crate-root re-exports.

### `src/lib.rs` module block
```rust
#[cfg(feature = "read")]
mod api;
mod binding;
#[cfg(feature = "read")]
mod capability;
mod db;
mod error;
#[cfg(feature = "read")]
pub mod protocol;
#[cfg(feature = "read")]
mod source;

#[cfg(all(test, feature = "server"))]
mod tests;
```
Plus flat `pub use` block (sources: `api::config`/`api::builder`/`api::runtime`/`capability::auth`/`capability::read`/`db::primary`/`db::shadow`/`error`) and `pub async fn init` delegating to `db::setup::prepare`.

### `src/api/mod.rs`
```rust
mod builder;
mod config;
mod runtime;
```
(lib.rs re-exports reach through `api::config::X` etc.; `config` items `pub(crate)` so `source/`/`db/` can import them.)

### `src/source/mod.rs`
`#[cfg(feature = "server")] mod replica; mod primary;` + the `pub(crate) async fn build_read_core` dispatcher matching once on `PgSource` (`Replica` arm `#[cfg(feature = "server")]`).

### `src/capability/mod.rs`
Ten `mod X;` lines (auth, cache, cdc, classify, diff, live, read, rows, schema, version). Parent `mod capability;` is read-gated in lib.rs.

### `src/binding/mod.rs`
`#[cfg(feature = "unb")] mod unb; #[cfg(feature = "server")] pub(crate) mod http;`

### `src/binding/http/mod.rs`
1:1 copy of current `src/http/mod.rs` (sibling `super::` refs in server.rs stay valid).

### `src/db/mod.rs`
`pub(crate) mod primary; #[cfg(feature = "server")] pub(crate) mod setup; pub(crate) mod shadow;` — `recover_primary`/`open_shadow`/`ShadowHandle` re-exported at crate root from these paths.

### `src/protocol/mod.rs`
`pub mod payload; pub mod subjects;`

## File Blueprint

### CREATE

| File | Content | Complexity | Phase |
|------|---------|------------|-------|
| `protocol/{mod,subjects,payload}.rs` | wire.rs split (3 consts / everything else + tests) | Low | 1 |
| `db/{mod,primary,shadow,setup}.rs` | whole-file moves; primary keeps exact ungated/gated split | Med | 2 |
| `capability/mod.rs` + 10 files | whole-file moves; `operations.rs`→`read.rs`; **preserve `is_private` complementary cfg arms** | High | 3 |
| `api/{mod,config,builder,runtime}.rs` | composition.rs split; renames; `SourceShutdown` → pub(crate); composition tests → runtime.rs with explicit `use crate::api::config::{PgSource, EmbeddedPrimarySource};` | High | 4 |
| `source/{mod,replica,primary}.rs` | assembly bodies + dispatcher (merged into Phase 4) | Med | 4 |
| `binding/{mod,unb}.rs`, `binding/http/*` | 1:1 moves | Low | 5 |

### MODIFY

| File | Edits | Phase |
|------|-------|-------|
| `src/lib.rs` | incrementally per phase; final reconcile + pub-use diff | 1–6 |
| `src/tests/mod.rs` | 4 import lines | 3 |
| `src/main.rs` | import renames + 2 call sites | 6 |
| `tests/primary.rs` | L1 imports, L11 wire→protocol, 11 rename sites | 7 |
| `integration-tests/src/lib.rs` (L145, L311), `unb_replica.rs` (L28), `bench/src/main.rs` (L261) | `pgpaw::Source::replica`→`pgpaw::PgSource::replica` | 7 |
| `README.md` | snippet/prose renames (~L268/275/289) + `target=` log examples (~L641-652) | 7 |

### DELETE (same phase as content moves + consumer repointing)

wire.rs (P1) · primary.rs, shadow.rs, setup.rs (P2) · classify/diff/cache/version/rows/schema/cdc/live/auth/operations.rs (P3) · composition.rs (P4) · http/ dir, unb.rs (P5)

## Implementation Phases

Exit gate for EVERY phase: 4-combo matrix green — `--no-default-features --features read` · default (`server`) · `--no-default-features --features unb` · `--all-features`; plus `cargo test --all-features --no-run` after test-moving phases (3, 4).

1. **protocol/**: split wire.rs; lib.rs `pub mod wire` → `pub mod protocol`; repoint unb.rs imports (file not yet moved); delete wire.rs. Gate: `git grep 'crate::wire'` empty.
2. **db/**: move shadow, setup, primary (keep every gate line); db/mod.rs re-exports; lib.rs repoints `recover_primary`/`open_shadow`; repoint composition.rs `crate::{primary,setup}::` calls. **R-new-1**: setup.rs/primary.rs keep temporary `crate::composition::{UpstreamConfig,PrimarySource}` imports until Phase 4 flips them to `crate::api::config::*`. Gate: read-only combo still exposes ungated recovery.
3. **capability/**: move 10 files (classify/diff/rows/schema/cache/version → cdc/live → auth → read last); keep duplicate `change_table`; capability/mod.rs; lib.rs repoints AuthConfig/ReadOperations/PreparedRead/HealthStatus; repoint composition.rs, unb.rs, http/*, src/tests/mod.rs imports; delete 10 files. Gate: stale-path grep empty + tests compile.
4. **api/ + source/ (one step)**: create api/config (renames + 8 gate rules), api/runtime (PgPaw + SourceShutdown pub(crate) + composition tests with renames), api/builder (open → `crate::source::build_read_core`, `crate::binding::*` paths — binding lands P5, so builder's binding paths flip in P5; alternatively keep `crate::http`/`crate::unb` paths until P5); create source/{mod,replica,primary}; flip db temp imports (4.6); lib.rs repoints + renames; delete composition.rs. Gate: `git grep 'crate::composition\|Self::build_read_core'` empty; tests compile.
5. **binding/**: move http/ 1:1 + unb.rs; binding/mod.rs; lib.rs `mod binding;`; flip api/builder.rs binding paths; delete src/http/, src/unb.rs. Gate: `git grep 'crate::http::\|crate::unb::'` empty. (R-new-2: `#[handler]` macro is signature-level, unaffected by file location — verify with all-features build.)
6. **crate-root reconcile**: finalize lib.rs; **diff old vs new `pub use` symbol set** (identical except PgSource/EmbeddedPrimarySource renames + wire→protocol); main.rs renames. Gate: `cargo test --all-features` PASSES (first full test run incl. `--exact tests::shutdown_signal_helper`).
7. **External consumers + README**: tests/primary.rs, integration-tests (2+1 sites), bench (1 site), README snippets + `target=` examples. Gate: `cargo test --workspace --all-features` passes; final grep for `pgpaw::wire|pgpaw::Source|::PrimarySource|pgpaw::http::|pgpaw::operations` returns nothing.
8. **Verification**: full matrix + workspace tests + `cargo clippy --all-features -- -D warnings` + `cargo fmt --check` (R-new-4).

## Spec Recommendation

Single new domain `module-architecture` with structural-invariant requirements (module ownership + dependency DAG, flat re-exports + protocol-only nested public module, behavior preservation incl. feature matrix / log `event=` stability / ungated recovery, type renames). No behavior scenarios.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| R1: `pgpaw::wire` public rename | intended | Med | update tests/primary.rs:11; no shim |
| R2: `db → capability::cdc` edge | pre-existing | Low | accepted; do not invert; keep `change_table` duplication |
| R3: log `target=` changes with `module_path!()` | certain | Med | unavoidable; no explicit `target:` params (scope creep); update README examples; migration note in change docs |
| R4: cfg-gate omission in the two dense splits | Med | High | per-line gate checklists (analysis §2); 4-combo matrix after every phase |
| R5: missed flat re-export | Low | High | explicit pub-use symbol-set diff (task 6.2) |
| R6: `--exact tests::shutdown_signal_helper` | none | none | main.rs untouched |
| R7: placement re-litigation | Low | Low | resolutions authoritative |
| R-new-1: db/ references api::config before it exists | Med | Med | park temp `crate::composition::*` imports in P2, flip in P4 |
| R-new-2: `#[handler]` macro path assumptions | Low | Med | signature-level macro; verify all-features build in P5 |
| R-new-3: schemars derive module-path sensitivity | Low | Low | derives use type names; existing wire tests re-verify post-move |
| R-new-4: no rustfmt/clippy configs exist | Low | Low | `cargo fmt --check` + clippy `-D warnings` at P8 |

## Open Questions

- Phase 4 is one larger step (api+source merged) — forced by mutual references; splitting requires a shim (rejected).
- Add a migration note for downstream log consumers keyed on `target=` (recommend yes, doc-only).
- `capability/mod.rs` per-child read gates are redundant under the read-gated parent — kept for style parity with current explicit per-item gating.

## Confidence Assessment

- Design completeness: 93 — all placement-relevant files read in full; the one judgment call (#4) resolved with defended rationale.
- Risk assessment: 90 — R1–R7 re-validated; R-new-1 (api↔db import ordering trap) is the substantive addition with a concrete park-and-flip plan.
- Implementation feasibility: 95 — all moved items are pub(crate) or re-exported; bottom-up sequence keeps every intermediate state green with zero shims.
