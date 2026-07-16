# Design: module-architecture-reshape

## Overview

Pure structural change: dissolve `composition.rs` and re-home every `src/` file into six ownership-scoped modules, with two type renames and one public path rename. No body edits — moves are copy-paste plus import-path and rename rewrites. The compiler plus a 4-combo feature matrix per phase is the safety net.

## Architecture

```
CLI (main.rs)
  └─ pgpaw:: flat re-exports (lib.rs)
       api/        config.rs (PgSource, EmbeddedPrimarySource, ReplicaSource, UpstreamConfig,
                    CacheConfig, HttpConfig, AzWireConfig)
                   builder.rs (PgPawBuilder → open)
                   runtime.rs (PgPaw, SourceShutdown, wait/shutdown/abort_open + lifecycle tests)
       source/     mod.rs (build_read_core dispatcher) · replica.rs · primary.rs
       capability/ read.rs (ex operations.rs) · auth · cache · live · cdc · version ·
                   schema · rows · classify · diff
       binding/    http/{server,query,health} · az_wire.rs
       db/         primary.rs (open/recovery/PrimaryObserver) · shadow.rs · setup.rs
       protocol/   payload.rs · subjects.rs   (public: pgpaw::protocol::*)
       error.rs    (unchanged, crate root)
```

## Key Decisions

### Decision 1: This layout deliberately overrides CLAUDE.md's "minimize new modules"

**Context:** CLAUDE.md says maximize cohesion, minimize new modules. This reshape creates 7 directories and ~25 files.
**Decision:** User-decided exception, recorded in CONTEXT.md's module-ownership table with an explicit "do not re-merge for cohesion's sake" note. Ownership boundaries beat file-count minimization for this codebase's current size (~4.3k lines, three feature tiers, two sources × two bindings).

### Decision 2: `build_read_core` becomes a free fn in `source/mod.rs`

**Context:** Currently `impl PgPawBuilder`; CONTEXT.md assigns source assembly to `source/`.
**Options:** stay on builder (drags assembly imports into `api/`), attach to `PgSource` (collapses config/assembly split, forces `api/config.rs` to import PGlite/ReadOperations), free fn beside the two assembly bodies.
**Decision:** `pub(crate) async fn build_read_core` in `source/mod.rs`, next to `build_replica_core`/`build_primary_core` (also `pub(crate)` free fns — they return 4-tuples; CLAUDE.md prefers tuples over one-off host structs). Struct-First yields to Least New Definitions + Strict Placement.

### Decision 3: Incremental bottom-up migration, zero shims

**Context:** Big-bang is compile-viable (everything is `pub(crate)` or re-exported) but the first 4-combo matrix run would land after ~20 files moved — a dropped cfg gate means bisecting the whole reshape.
**Decision:** Move leaves first (protocol → db → capability → api+source → binding → root), repointing each layer's consumers in the same step so old/new paths never coexist — no temporary re-export shims to add and later forget. Phase 4 merges `api/` + `source/` (mutually referential via `build_read_core`/`SourceShutdown`). One wrinkle (R-new-1): `db/` lands before `api/config` exists, so `db/setup.rs`/`db/primary.rs` keep temporary `crate::composition::*` imports for two phases, flipped in task 4.6.

### Decision 4: `pgpaw::wire` → `pgpaw::protocol::{payload,subjects}` is a real public rename

**Context:** `wire` is today the only nested public module; the target tree replaces it.
**Decision:** Intentional in-scope break; sole first-party consumer (`tests/primary.rs:11`) updated in the same change; no compat shim. `protocol/mod.rs` is the only new `pub mod`.

### Decision 5: Placement calls

- `AuthConfig` → `capability/auth.rs` (carries `into_verifier` behavior coupled to `Verifier`; flat re-export hides the location; user's own tree annotation marked it `AuthConfig?`).
- `PrimaryObserver` → `db/primary.rs` (user tree annotation "observer primitives"), accepting the pre-existing `db → capability::cdc` edge — not to be inverted (behavior refactor, out of scope).
- `SourceShutdown` → `api/runtime.rs` (user tree annotation), visibility widened private → `pub(crate)` so the source dispatcher can name it in its return type. Only two visibility widenings in the whole change (the other: `UpstreamConfig::sslmode()` → `pub(crate)`).

### Decision 6: Accepted side effect — log `target=` values change

`module_path!()`-derived targets shift (`pgpaw::operations` → `pgpaw::capability::read`, `pgpaw::http::query` → `pgpaw::binding::http::query`, ...). `event=` names stay byte-identical. Adding explicit `target:` params to every log call to preserve old values would be scope creep — rejected. README log examples updated + a migration note for anyone parsing `target=`.

## API Changes

Public surface identical except:
- `Source` → `PgSource`, `PrimarySource` → `EmbeddedPrimarySource` (token renames)
- `pgpaw::wire::{ReadRequest, ..., READ_SUBJECT, ...}` → `pgpaw::protocol::payload::{..}` / `pgpaw::protocol::subjects::{..}`

Completion gate: diff of the old vs new `pub use` symbol set must show exactly these differences and nothing else.

## Data Model / Security Considerations

None. No struct gains/loses fields; no semantics change. Auth/RLS, cache, replication, CDC, HTTP status mapping, az-wire error mapping all byte-identical — enforced by the "no body edits" rule and the full pre-existing test suite passing unmodified (beyond path/rename updates).
