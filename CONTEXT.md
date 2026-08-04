# CONTEXT

Vocabulary for PgPaw. Use these terms exactly in specs, proposals, and code discussion.

## Terms

- **PgSource** (formerly `Source`) — where PgPaw's data comes from. Exactly one per instance. Two variants: `PgSource::replica(ReplicaSource)` (logical replication follower of an upstream Postgres) and `PgSource::primary(EmbeddedPrimarySource)` (embedded writable Postgres owned by PgPaw). The source determines replica vs primary behavior; there is no separate mode switch.
- **EmbeddedPrimarySource** (formerly `PrimarySource`) — config for the embedded writable primary. `ReplicaSource` keeps its name (already unambiguous).
- **Binding** — a way clients reach PgPaw's read/live capabilities. Optional, zero or more. Current bindings: HTTP (`HttpConfig`) and unb (`UnbConfig`). Any binding works with any source.
- **Capability** — a read-side semantic unit (read operations, auth, cache, live, cdc, version, classify, diff, rows, schema). Owned by `capability/`; consumed by bindings; source-agnostic.
- **Read core** — the assembled capability stack shared by all bindings: `ReadOperations`, `LiveHub`, `QueryCache`, `VersionIndex`, auth `Verifier`. Built once from the source, handed to each binding.
- **PgPaw (runtime)** — the struct returned by `PgPawBuilder::open()`. Owns the read core and all started bindings. Exposes `wait()`, `shutdown()`, `primary_dsn()`. Instance-based: no global singleton.
- **PgPawBuilder** — the only composition entry point: `PgPaw::builder()`. Collects source, cache, auth, optional HTTP, zero-or-more unb configs, then `open()`.
- **UnbConfig** — pair of `unb::NodeBuilder` + `unb::TopologyConfig` passed through verbatim. PgPaw never invents unb binding options.

## Module ownership (post module-reshape)

- `api/` — public builder/runtime types (`config.rs`, `builder.rs`, `runtime.rs`). Flat re-exports at crate root: consumers write `pgpaw::{PgPaw, PgSource, ..}`, never `pgpaw::api::..`.
- `source/` — opens local database state and CDC for either replica or embedded primary (source assembly).
- `capability/` — read/cache/live/auth semantics.
- `binding/` — exposes capabilities over HTTP (`binding/http/`) or unb (`binding/unb.rs`).
- `db/` — low-level pglite setup/open/recovery/observer primitives (+ shadow, upstream setup).
- `protocol/` — PgPaw subject constants and request/response payload structs.
- `main.rs` — CLI adapter only: maps options to builder calls.
- One architectural responsibility per module. This layout is a deliberate, user-decided exception to the "minimize new modules" default in CLAUDE.md — do not re-merge these modules for cohesion's sake.

## Rules

- Lib owns PgPaw composition. unb crate owns unb topology. CLI only maps flags to the lib builder.
