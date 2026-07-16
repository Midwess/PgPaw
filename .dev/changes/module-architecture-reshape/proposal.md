# Proposal: Module Architecture Reshape

**Status**: approved

## Summary

Reshape PgPaw's `src/` tree around ownership boundaries — `api/`, `source/`, `capability/`, `binding/`, `db/`, `protocol/` — with zero behavior change: dissolve the `composition.rs` god module, relocate every file per the ownership table, rename `Source`→`PgSource` and `PrimarySource`→`EmbeddedPrimarySource`, and keep flat crate-root re-exports so consumers only touch renames.

## Motivation

The unified-builder-api change produced a conceptually correct API but left `src/composition.rs` as a ~670-line god module mixing seven responsibilities: public config types, replica source assembly, primary source assembly, HTTP binding startup, az-wire binding startup, runtime wait/shutdown/rollback, and lifecycle tests. Other names are stale (`operations.rs` hosts the read capability, `wire.rs` hosts the az-wire protocol payloads) or too generic (`Source`). Flat module list gives no ownership signal — a reader cannot tell which files may know about actix, which about pglite recovery, and which are pure semantics.

Target: `CLI flags → api/ builder → source/ assembly → capability/ read core → binding/ exposure`, with `db/` (pglite primitives) and `protocol/` (subjects + payloads) as leaves. One architectural responsibility per module.

This layout is a deliberate, user-decided exception to CLAUDE.md's "minimize new modules" default — recorded in CONTEXT.md so future changes don't re-merge the tree for cohesion's sake.

## Scope

### In Scope

- Move/split every `src/` file per the item-by-item move map in analysis.md (§1): `composition.rs` → `api/{config,builder,runtime}.rs` + `source/{mod,replica,primary}.rs`; read-side modules → `capability/*`; `http/*` + `az_wire.rs` → `binding/*`; `primary.rs`/`shadow.rs`/`setup.rs` → `db/*`; `wire.rs` → `protocol/{payload,subjects}.rs`
- Renames: `Source` → `PgSource`, `PrimarySource` → `EmbeddedPrimarySource`, public `pgpaw::wire` → `pgpaw::protocol::{payload,subjects}`
- Preserve flat crate-root re-exports (`pgpaw::{PgPaw, PgSource, ...}`) and the exact feature-gate matrix (`read`/`server`/`az-wire`) per the gate-redistribution table
- Update all consumers: `src/main.rs`, `tests/primary.rs`, `integration-tests` (2+1 call sites), `bench` (1 call site), `src/tests/mod.rs` (4 imports), README examples
- `src/error.rs` and `src/main.rs` stay at their current paths

### Out of Scope

- Any behavior change: SQL, replication, trigger CDC, auth/RLS, cache semantics, HTTP responses, az-wire behavior, CLI behavior, logfmt `event=` names
- az-wire topology ownership (still accepts `az_wire::NodeBuilder` + `TopologyConfig` verbatim)
- New abstractions beyond what file-splitting strictly requires; no dedup of the pre-existing `change_table` duplication (live.rs/version.rs)
- Inverting the pre-existing `db::primary::PrimaryObserver → capability::cdc::CdcBridge` dependency edge
- Renaming `ReadOperations` or `ReplicaSource`

## Affected Areas

| Area | Impact |
|------|--------|
| `src/composition.rs` | Deleted — split across `api/*` and `source/*` |
| `src/lib.rs` | Rewritten: new module tree + flat re-exports + `init()` |
| `src/{auth,operations,cdc,cache,live,version,rows,schema,classify,diff}.rs` | Moved to `capability/*` (operations → `read.rs`) |
| `src/{primary,shadow,setup}.rs` | Moved to `db/*` |
| `src/wire.rs` | Split into `protocol/{payload,subjects}.rs` — public path renames |
| `src/http/*`, `src/az_wire.rs` | Moved to `binding/http/*`, `binding/az_wire.rs` |
| `src/main.rs` | Rename-only edits (import list + 2 call sites); stays at path |
| `src/tests/mod.rs` | 4 import-line updates; stays at path |
| `tests/primary.rs` | Import + 11 rename sites; `pgpaw::wire` → `pgpaw::protocol` |
| `integration-tests`, `bench` | 4 one-line rename sites total |
| `README.md` | Snippet renames + `target=` log-example updates |

## Dependencies

- None external. Prerequisite (strongly recommended): commit the current working tree (pre-existing work + unified-builder-api) first — a whole-tree file reshape stacked on an uncommitted feature change makes the combined diff unreviewable and loses `git mv` rename detection.

## Risks

| Risk | Mitigation |
|------|------------|
| cfg-gate omission during the two dense splits (`composition.rs`, `operations.rs`) — a dropped/flipped `#[cfg]` arm silently changes behavior or breaks a feature combo | Gate-redistribution tables in analysis.md §2 followed line-by-line; build all 4 feature combos after every phase (precedent: commit b3f3038) |
| Missed flat re-export changes public API | Diff old vs new `pub use` list as an explicit completion gate |
| `pgpaw::wire` public path rename breaks consumers | Only first-party consumer (`tests/primary.rs:11`) — updated in the same change; intentional rename |
| log `target=` values change with `module_path!()` (`pgpaw::operations` → `pgpaw::capability::read`) | Unavoidable in a module move; `event=` names byte-identical; README log examples updated; noted for anyone parsing `target=` |
| Reshape on a dirty tree destroys rename detection and review-ability | Commit current state before applying |
| "While I'm here" edits sneaking into moves | Surgical rule: moves are copy-paste + path edits only; review gate checks for any non-path diff inside moved bodies |
