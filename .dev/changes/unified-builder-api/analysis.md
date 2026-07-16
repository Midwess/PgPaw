# Codebase Analysis: unified-builder-api

Generated: 2026-07-16
Scope: Replace PgPaw's three fragmented entrypoints (`run`/`run_until`, `open_primary`/`run_primary`, `register_az_wire`) with one `PgPaw::builder()` composition API — single `Source` (replica|primary) + optional bindings (HTTP, az-wire) + instance-based runtime, no global singleton, hard-cut of old API, CLI reduced to flag→builder mapping.

## Project Context
- **Tech Stack**: Rust 1.85, actix-web 4, tokio, `pglite-rs` (path dep on `../pglite-rs/crates/pglite`, features `replica` + `multiple-process`), az-wire (path dep, workspace member excluded from `[workspace]`), sqlparser 0.52, moka 0.12, serde/serde_json, thiserror 2, jsonwebtoken 9.
- **Architecture Style**: Single-crate layered lib + thin CLI. Feature-gated modules (`read`, `server`, `az-wire`) compose a "read core" that is fed to zero-or-more "bindings" (HTTP, az-wire). Today the read core is built by two separate, divergent code paths (`Di::init` for replica, `PrimaryHandle::attach_child` for primary) rather than one shared constructor.
- **Key Directories**: `src/lib.rs` (entrypoints), `src/di.rs` (singleton + replica read-core construction), `src/primary.rs` (primary lifecycle + primary read-core construction + `PrimaryObserver`), `src/az_wire.rs` (az-wire service registration), `src/http/*` (actix handlers), `src/operations.rs` (`ReadOperations`, the read core's façade), `src/main.rs` (clap CLI).

## Similar Features Found

### 1. Primary's `attach_child` (src/primary.rs:86-144)
- **Pattern**: Builds a second, parallel "read core" (schema scan → `VersionIndex` → `CdcBridge::primary` → `LiveHub::start` → `Verifier::build` → `ReadOperations::primary`) then wires it directly into az-wire via `crate::register_az_wire(...).build()` and `start_topology`. Closest existing precedent for "build read core once, hand to a binding" — but primary-only and az-wire-only; no HTTP binding over a primary today.
- **Relevance**: Template for the unified builder's "build read core from source, then start bindings" flow. The builder must generalize this so the same construction path also produces a core usable by HTTP, and so replica sources go through the same shape instead of `Di::init`.

### 2. `Di::init` (src/di.rs:61-159)
- **Pattern**: Builds the read core from `ServerConfig`+`UpstreamConfig` (preflight → `PGlite::open_multi_process` → `Replica::start` → schema scan → `VersionIndex::new` → `CdcBridge::start(&replica, ...)` → `LiveHub::start` → `QueryCache::new` → `Verifier::build` → `ReadOperations::new`), then stores the whole `Di` struct into a `'static OnceCell`.
- **Relevance**: The replica-side read-core construction the builder must reconcile with `attach_child`'s primary-side construction. `Di` bundles config-derived HTTP fields (`bind_addr`, `cors_origin`) into the same struct as the read core — those need to move to the HTTP binding config, not the read core.

### 3. `run_until` (src/lib.rs:56-166)
- **Pattern**: Full server lifecycle: `Di::init` → bind HTTP → optionally build+start az-wire topology → `tokio::select!` race between shutdown signal, HTTP future, and topology `.wait()` → ordered shutdown (HTTP `handle.stop(true)` → await HTTP future if not finished → topology `.shutdown()` → `Di::instance().shutdown()`).
- **Relevance**: Primary template for the new `PgPaw` runtime's `wait()`/`shutdown()` behavior — the select/shutdown-ordering logic moves onto the `PgPaw` struct, generalized to `Vec<AzWireConfig>` (repeatable) and optional HTTP untied from `Di`.

## Architecture Layers

| Layer | Directory/File | Pattern | Examples |
|-------|-----------|---------|----------|
| CLI | `src/main.rs` | clap `#[arg(long, env)]` structs, `.config()` methods converting CLI options → lib config structs | `ServeOptions::config() -> ServerConfig`, `PrimaryOptions::config() -> PrimaryConfig` |
| Composition/lifecycle | `src/lib.rs` | free async fns (`run`, `run_until`, `init`) orchestrating startup/shutdown | `run_until<F>(config, shutdown)` |
| Global state (to be removed) | `src/di.rs` | `OnceCell<Di>` singleton, `Di::instance()` accessor | `static INSTANCE: OnceCell<Di>` |
| Primary lifecycle | `src/primary.rs` | struct-owned handle (`PrimaryHandle`) with async methods, no singleton | `open_primary`, `PrimaryHandle::attach_child`, `PrimaryObserver` |
| Read core façade | `src/operations.rs` | single `ReadOperations` struct with two constructors (`new` for replica/server, `primary` for embedded primary), `#[derive(Clone)]` with per-field `Arc`/`Mutex` | `ReadOperations::new(...)`, `ReadOperations::primary(...)` |
| CDC | `src/cdc.rs` | single `CdcBridge` struct with two constructors (`start` from `Replica::subscribe()`, `primary` fed via mpsc + `publish`) | `CdcBridge::start(&replica, versions)`, `CdcBridge::primary(versions)` |
| Live/SSE | `src/live.rs` | `LiveHub::start(bridge, db, pk)` — source-agnostic, takes a `CdcBridge` reference regardless of origin | `LiveHub::start` |
| Cache | `src/cache.rs` | `QueryCache` — moka wrapper, source-agnostic | `QueryCache::new(max_bytes)` |
| Version tracking | `src/version.rs` | `VersionIndex` — source-agnostic | `VersionIndex::new(pk, full)` |
| Auth | `src/auth.rs` | `Verifier::build(...)`, `Principal`; `AuthOutcome` actix `FromRequest` impl reaches into `Di::instance()` | `Verifier::build`, `authenticate()` free fn |
| HTTP binding | `src/http/{server,query,health}.rs` | route handlers as free async fns reading `Di::instance()` directly (no `web::Data`) | `query()`, `healthz()`, `bind()` |
| az-wire binding | `src/az_wire.rs` | `register_az_wire(builder, operations) -> NodeBuilder`, handler fns via `#[handler]` macro reading `State<ReadOperations>` | `register_az_wire`, `read`/`cursor`/`live` handlers |
| Errors | `src/error.rs` | single `thiserror::Error` enum `CacheError` + `LifecycleErrorKind` for categorized lifecycle errors | `CacheError::lifecycle(kind, error)` |
| Ephemeral primary | `src/shadow.rs` | `open_shadow()` — out of scope | `ShadowHandle` |

## Dependencies for Target Area

### Internal Dependencies
- `ReadOperations` (src/operations.rs): read core façade every binding calls into (`prepare`, `materialize`, `execute_private`, `subscribe`, `cursor`, `authenticate`). Two constructors to reconcile.
- `LiveHub`, `QueryCache`, `VersionIndex`: already source-agnostic.
- `Verifier` (src/auth.rs): source-agnostic build, but `AuthOutcome`'s `FromRequest` impl (src/auth.rs:105-112, 128) hardwired to `Di::instance()` — must become `web::Data`-based.
- `CdcBridge` (src/cdc.rs): `start(&Replica, versions)` (replica; thread reads `replica.subscribe()`) vs `primary(versions)` (mpsc fed by `publish()` from `PrimaryObserver` LISTEN/NOTIFY callback). Same `subscribe()`/`stop()` surface — the direct precedent for how `Source` variants feed one internal read-core builder.
- `crate::schema::scan_schema(&db)`: used identically by both paths — fully source-agnostic.

### External Dependencies
- `pglite-rs` (`PGlite`, `Replica`, `ReplicaConfig`, `MultiProcessOptions`, `SslMode`): `PGlite::open_multi_process` used by both sources; only replica calls `Replica::start`.
- `az_wire::NodeBuilder`/`TopologyConfig`/`HostConfig`: `AzWireConfig` wraps verbatim. Topology start (`build()` + `start_topology`) duplicated between `lib.rs::run_until` and `primary.rs::attach_child` with different error mapping (`CacheError::Config` vs `CacheError::lifecycle(LifecycleErrorKind::Topology, ...)`) — unified runtime should standardize on the lifecycle mapping.
- `actix_web`/`actix_cors`: HTTP binding only; `HttpServer::new` closure captures `cors_origin` by move (src/http/server.rs:16) — reusable pattern for `web::Data` migration.
- `tokio_postgres`: upstream preflight/setup only (replica path).

### Configuration Dependencies
- `ServerConfig`/`UpstreamConfig` (src/di.rs:18-44) — deleted; fields redistributed across `Source::replica(ReplicaSource)`, `HttpConfig`, `CacheConfig`, `AuthConfig`, `AzWireConfig`.
- `PrimaryConfig` (src/primary.rs:10-19) — becomes the substance of `Source::primary(PrimarySource)`; `EmbeddedVerifierConfig` deleted — JWT fields need a source/binding-independent home (`AuthConfig`) since HTTP-on-primary is in scope.

### Data Dependencies
- Embedded pglite `data_dir` — same for both sources.
- Catalog scans (schema.rs, operations.rs `classify_security`) — identical both paths.
- LISTEN/NOTIFY channel `pgpaw_primary_{pid}` (src/primary.rs:401) — primary-only CDC substitute; stays internal to primary source construction.

## Execution Flow

**Replica + HTTP (current `run_until`)**
1. `lib.rs::run_until` → `Di::init(config)`.
2. `Di::init`: preflight → open pglite → `Replica::start` → scan_schema → `VersionIndex::new` → `CdcBridge::start` → `LiveHub::start` → `QueryCache::new` → `Verifier::build` → `ReadOperations::new` → `INSTANCE.set(di)`.
3. `http::server::bind()` reads `Di::instance().bind_addr()`/`cors_origin()`.
4. Optional az-wire: `register_az_wire(NodeBuilder, Di::instance().operations().clone())` → `.build()` → `.start_topology(...)`.
5. `tokio::select!` biased over shutdown signal / HTTP future / topology `.wait()`.
6. Shutdown: HTTP `handle.stop(true)` → await HTTP future → topology `.shutdown()` → `Di::instance().shutdown()` (replica.stop → cdc.stop → db.shutdown).

**Primary + az-wire (current `attach_child`)**
1. `run_primary(config)` → `open_primary(&config)` → `PrimaryHandle`.
2. Read-core build deferred to `attach_child(node, topology_config)`: validate listenerless-parent topology → scan_schema → `VersionIndex::new` → `CdcBridge::primary` → `LiveHub::start` → `Verifier::build` from `EmbeddedVerifierConfig` → `ReadOperations::primary` → `PrimaryObserver::start` (pg_notify triggers on every table, LISTEN, on notify `bridge.publish(CommittedTransaction{..Truncate})` to force full row diff — primary has no WAL stream).
3. `register_az_wire(...).build()` → `start_topology(topology)`.
4. Shutdown: topology → observer (cdc stop, unlisten) → `db.close()`.

**Divergence the unified read core must reconcile**: replica builds the read core eagerly inside `Di::init` (config → core, one shot); primary defers it to `attach_child`, only when az-wire is requested. The unified builder needs read-core construction as a single step, decoupled from any binding, running during `.open()` regardless of bindings.

## Conventions to Follow

| Category | Convention |
|----------|------------|
| File naming | one file per domain concept (`operations.rs`, `cdc.rs`, `version.rs`, `live.rs`) |
| Constructors | named for source/mode instead of generic overloads (`ReadOperations::new` vs `::primary`; `CdcBridge::start` vs `::primary`) |
| Testing | `#[cfg(test)] mod tests` inline at file bottom; integration tests in `tests/` one file per domain; `#[serial_test::serial]` on tests binding real ports/data dirs |
| Errors | single `CacheError` thiserror enum; lifecycle errors via `CacheError::lifecycle(LifecycleErrorKind::X, source)`; `?` everywhere |
| Logging | `log::info!` logfmt-style `event=x key=value`, one line per lifecycle milestone |
| Feature gating | `server` for actix code, `az-wire` for az-wire code, `read` for read-core tree; layering `az-wire ⊃ read`, `server ⊃ read` |
| Locks | `Arc<Mutex<T>>` fields directly on `#[derive(Clone)]` structs, brief scopes in `&self` methods, no `Inner` wrappers |
| CLI mapping | clap options structs + `.config()` conversion methods |
| Comments | none — zero inline comments throughout |

## OpenSpec Integration Notes

- **Affected domains**: architectural/composition-layer change cutting across lifecycle/composition (`lib.rs`, `di.rs`, `primary.rs`, `az_wire.rs`), HTTP binding (`http/*`), auth (`AuthOutcome` `FromRequest`), CLI (`main.rs`). Does NOT touch `classify.rs`, `version.rs`, `cache.rs`, `live.rs`, `diff.rs`, `rows.rs`, `wire.rs`, `shadow.rs` internals.
- **Existing specs to preserve**: `.dev/changes/jwt-access-control/specs/access-control/spec.md` (`Verifier`/private-query semantics must survive the new construction path) and `.dev/changes/tanstack-db-live-sync/specs/realtime/spec.md` (live/SSE wire semantics preserved verbatim — `LiveHub`/`CdcBridge` internals unchanged).
- **Spec structure**: delta domains `composition` (builder surface, `Source` variants, binding accumulation, shutdown ordering) and `cli` (flag→builder mapping table); reference — not duplicate — access-control and realtime specs for read-core behavior.

## Risks and Considerations

| Risk | Impact | Mitigation |
|------|--------|------------|
| `Di` singleton removal touches actix `FromRequest` (`AuthOutcome`) and all three HTTP handlers (9 `Di::instance()` call sites: auth.rs:128, http/health.rs:7, http/query.rs:26/136, http/server.rs:9-10, lib.rs:84/94/109/118/163) | Compile-time blast radius across `src/http/*` and `src/auth.rs` | State passed via `web::Data<T>` injected into `App::new()` per binding; handler signatures accept `web::Data<T>` |
| `healthz` (src/http/health.rs) calls `Di::instance().replica()` — HTTP can now run over a primary source with no `Replica` | halted/watermark health semantics don't exist for primary | Read core exposes source-agnostic health concept, or healthz behavior branches per source — design decision needed (flagged for design.md) |
| Two constructor families (`new`/`start` vs `primary`) must reconcile without merely relocating the branch | Naive branching per binding doesn't unify anything | Builder branches once on `Source` early (produce event source + optional replica), then one shared assembly step for `VersionIndex`/`QueryCache`/`LiveHub`/`Verifier`/`ReadOperations` — Least New Definitions: reuse existing dual constructors, builder is the single call site |
| `EmbeddedVerifierConfig` deletion removes the only JWT-config path for primary sources, but HTTP-on-primary now supported | Primary + HTTP would lose auth | `AuthConfig` becomes source/binding-independent builder input consumed by the unified read-core assembly |
| `register_az_wire` goes private but its registration logic is still needed | Must not break az-wire binding | Keep function body, drop `pub` + `pub use` re-export |
| Shutdown ordering differs between paths (server: HTTP→topology→replica/cdc/db; primary: topology→observer/cdc→db.close) | Unified `shutdown()`/`wait()` needs one canonical ordering correct for all combos, incl. HTTP-on-primary (no precedent) | Generalize: stop all bindings first (HTTP then az-wire topologies), then read-core internals (cdc/observer stop, replica.stop if replica, then db close) |
| `tests/primary.rs` (390 lines) + `src/az_wire.rs` test module use `open_primary`/`attach_child`/`EmbeddedVerifierConfig` directly | Near-total rewrite of those tests | Rewrite against `PgPawBuilder`/`PgPaw`; `recover_primary` tests stay (API kept); `tests/topology_benchmark.rs` uses raw `az_wire::Node`, unaffected |
| CLI structs map onto deleted config types | Full rewrite of `.config()` glue | `Serve`/`Primary` subcommands stay (per dream-shape CLI), each mapping flags to one builder call; `Init` unchanged |

## Confidence Assessment

- **Pattern confidence**: 90 — dual-constructor pattern directly observed; clear precedent.
- **Architecture understanding**: 85 — both startup/shutdown paths traced end to end incl. all 9 `Di::instance()` call sites. Gap: no precedent for HTTP-over-primary health checking.
- **Recommendation confidence**: 80 — read-core unification and shutdown ordering generalize directly from code; healthz-over-primary needs a design decision in design.md.
