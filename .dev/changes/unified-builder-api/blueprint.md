# Architecture Blueprint: unified-builder-api

Generated: 2026-07-16
Based on: `.dev/changes/unified-builder-api/analysis.md`

## Design Summary

Replace the three divergent entrypoints (`run`/`run_until`, `open_primary`/`run_primary`/`attach_child`, `register_az_wire`) and the `Di` `OnceCell` singleton with a single instance-based composition API: `PgPaw::builder()` collects one `Source` (replica|primary), a `CacheConfig`, an `AuthConfig`, an optional `HttpConfig`, and zero-or-more `AzWireConfig`, then `.open()` builds one shared read core (via a single internal assembly branching once on `Source`) and starts every requested binding. HTTP handlers and `AuthOutcome` receive state through actix `web::Data` instead of the global. The runtime handle `PgPaw` owns the read core plus all started bindings and exposes `wait()`, `shutdown()`, `primary_dsn()`.

az-wire API facts verified against source: `AzWireTopology::wait(&mut self)`, `shutdown(self)`; `NodeBuilder::build() -> Node`, `Node::start_topology(TopologyConfig) -> Result<AzWireTopology, WsError>`; `TopologyConfig { host: Option<HostConfig>, parent: Option<ParentLink> }`.

## Design Decisions

| # | Decision | Chosen | Rationale |
|---|----------|--------|-----------|
| a | `healthz` over any source | `healthz` reads `web::Data<ReadOperations>` and calls new `ReadOperations::health() -> HealthStatus`. Replica source returns `{status, halt_reason, watermark}`; primary source (no `Replica`) returns `{status:"ok"}` unconditionally. | Least New Definitions: `ReadOperations` already holds `Option<Replica>` and is the read-core façade every binding calls. One small method + return struct beats a per-source handler branch. Halted/watermark semantics preserved verbatim for replica. |
| b | Source-specific state in `PgPaw` runtime | `PgPaw` holds flat shared fields (`read: ReadOperations`, `db: PGlite`, `dsn: Option<String>`, `http: Option<Server>`, `az_wire: Vec<AzWireTopology>`) plus `SourceShutdown` enum owning only source-specific teardown handles (`Replica{replica, cdc}` vs `Primary{observer}`). | `SourceShutdown` is the single genuinely-new domain concept (source teardown differs and must be typed). No fake `Inner` struct; the enum carries behavior (per-source shutdown ordering). `dsn` is naturally `Option` (primary-only). |
| c | Canonical shutdown ordering | Bindings first, then core: (1) HTTP `handle.stop(true)` + await server future; (2) each `AzWireTopology::shutdown()` in `Vec` order; (3) source teardown: replica → `replica.stop()` + `cdc.stop()`; primary → `observer.shutdown(&db)`; (4) `db.shutdown()`/`db.close()` last. | Correct for every source×binding combo incl. HTTP-on-primary. Standardizes error mapping on `CacheError::lifecycle(LifecycleErrorKind::Shutdown, _)` (the two old paths diverged on `Config` vs `lifecycle`). |
| d | `wait()` semantics | With ≥1 binding: `tokio::select!` biased over HTTP server future and each topology `.wait()`; first completion (fatal or clean) wins and triggers shutdown. With zero bindings (embedded primary, dsn-only): `wait()` awaits `std::future::pending()`. Signal handling stays in `main.rs`. | Matches `run_primary`'s current `pending()` semantics for dsn-only use and `run_until`'s select for bound servers; keeps signals out of the lib. |
| e | `AuthConfig` shape | `AuthConfig { jwt_secret, jwt_public_key, jwt_jwks_url: Option<String>, role_claim: Option<String> }` with `AuthConfig::none()` (== Default), `::jwt_secret(s)`, `::jwt_public_key(pem)`, `::jwt_jwks_url(url)` constructors, chainable `.role_claim(r)`, `pub(crate) fn into_verifier() -> Result<Option<Verifier>, CacheError>` delegating to unchanged `Verifier::build`. | Source/binding-independent home for JWT (required since HTTP-on-primary is in scope and `EmbeddedVerifierConfig` is deleted). Validation stays in `Verifier::build`. |
| f | Builder validation errors | At `.open()`: missing source → `CacheError::Config("PgPaw requires a source")`. `.http()`/`.az_wire()` without their feature are compile-gated (`#[cfg]`'d methods), never runtime errors. | Single fallible boundary keeps setters infallible and chainable, matching the dream API. |
| g | Struct placement | New `src/composition.rs` hosts `PgPawBuilder`, `PgPaw`, `Source`/`ReplicaSource`/`PrimarySource`, `UpstreamConfig` (relocated field-block), `CacheConfig`, `HttpConfig`, `AzWireConfig`, `SourceShutdown`, `build_read_core`. `AuthConfig` lives in `src/auth.rs` (adjacent to `Verifier`). `HealthStatus` + `health()` on `ReadOperations` in `operations.rs`. `src/di.rs` deleted. | One new file for the genuinely-new composition domain; net +1 domain file, −1 singleton file. Auth config attaches to the module it configures; health attaches to the façade it queries. |

## Component Designs

### `PgPawBuilder` (src/composition.rs)

```rust
pub struct PgPawBuilder {
    source: Option<Source>,
    cache: CacheConfig,
    auth: AuthConfig,
    #[cfg(feature = "server")]
    http: Option<HttpConfig>,
    #[cfg(feature = "az-wire")]
    az_wire: Vec<AzWireConfig>,
}

impl PgPawBuilder {
    pub fn source(self, source: Source) -> Self;
    pub fn cache(self, cache: CacheConfig) -> Self;
    pub fn auth(self, auth: AuthConfig) -> Self;
    #[cfg(feature = "server")]
    pub fn http(self, http: HttpConfig) -> Self;
    #[cfg(feature = "az-wire")]
    pub fn az_wire(self, node: az_wire::NodeBuilder, topology: az_wire::TopologyConfig) -> Self;
    pub async fn open(self) -> Result<PgPaw, CacheError>;
}
```

`open()` flow: validate `source.is_some()` → `build_read_core` → wrap `ReadOperations` in `web::Data` → start HTTP binding (if `http`) → start each az-wire topology → assemble `PgPaw`. On any start failure, tear down already-started bindings + read core before returning `Err` (mirrors current `run_until` rollback).

### `PgPaw` runtime handle (src/composition.rs)

```rust
pub struct PgPaw {
    read: ReadOperations,
    db: PGlite,
    dsn: Option<String>,
    shutdown_state: SourceShutdown,
    #[cfg(feature = "server")]
    http: Option<actix_web::dev::Server>,
    #[cfg(feature = "az-wire")]
    az_wire: Vec<az_wire::AzWireTopology>,
}

impl PgPaw {
    pub fn builder() -> PgPawBuilder;
    pub fn primary_dsn(&self) -> Option<&str>;
    pub async fn wait(&mut self) -> Result<(), CacheError>;
    pub async fn shutdown(self) -> Result<(), CacheError>;
}

enum SourceShutdown {
    Replica { replica: Replica, cdc: CdcBridge },
    Primary { observer: Option<PrimaryObserver> },
}
```

`wait(&mut self)` because `AzWireTopology::wait(&mut self)` and the actix `Server` future poll by `&mut`; callers bind `let mut pgpaw`. `shutdown(self)` consumes because `AzWireTopology::shutdown(self)` and `db.close()` consume.

### `Source` / `ReplicaSource` / `PrimarySource` (src/composition.rs)

```rust
pub enum Source {
    Replica(ReplicaSource),
    Primary(PrimarySource),
}
impl Source {
    pub fn replica(source: ReplicaSource) -> Source;
    pub fn primary(source: PrimarySource) -> Source;
}

pub struct ReplicaSource {
    pub upstream: UpstreamConfig,
    pub data_dir: PathBuf,
    pub publication: String,
    pub slot: String,
    pub max_connections: usize,
}

pub struct PrimarySource {
    pub data_dir: PathBuf,
    pub database: String,
    pub listen_addresses: String,
    pub port: u16,
    pub min_connections: usize,
    pub max_connections: usize,
}
impl PrimarySource {
    pub fn embedded(data_dir: impl Into<PathBuf>) -> PrimarySource;
}
```

`UpstreamConfig` keeps the transport/auth block (host/port/user/password/database/sslmode, sslmode defaulted `"disable"`); `publication`/`slot`/`max_connections` hoist to `ReplicaSource` per the dream API. `setup.rs` `preflight`/`prepare` keep taking `&UpstreamConfig` with publication/slot passed alongside or via `&ReplicaSource`.

### `CacheConfig` / `HttpConfig` / `AzWireConfig` (src/composition.rs)

```rust
pub struct CacheConfig { pub max_bytes: u64 }
impl Default for CacheConfig { /* 256 * 1024 * 1024 */ }

#[cfg(feature = "server")]
pub struct HttpConfig {
    pub addr: std::net::SocketAddr,
    pub cors_origin: Option<String>,
}

#[cfg(feature = "az-wire")]
pub struct AzWireConfig {
    node: az_wire::NodeBuilder,
    topology: az_wire::TopologyConfig,
}
```

`HttpConfig.addr` is a `SocketAddr` (dream API `.parse()?`), replacing `bind_addr: String`. `AzWireConfig` passed through verbatim; PgPaw only calls `register_az_wire(node, read.clone()).build()?.start_topology(topology)`.

### `AuthConfig` (src/auth.rs)

```rust
#[derive(Clone, Default)]
pub struct AuthConfig {
    jwt_secret: Option<String>,
    jwt_public_key: Option<String>,
    jwt_jwks_url: Option<String>,
    role_claim: Option<String>,
}
impl AuthConfig {
    pub fn none() -> AuthConfig;
    pub fn jwt_secret(secret: impl Into<String>) -> AuthConfig;
    pub fn jwt_public_key(pem: impl Into<String>) -> AuthConfig;
    pub fn jwt_jwks_url(url: impl Into<String>) -> AuthConfig;
    pub fn role_claim(self, claim: impl Into<String>) -> AuthConfig;
    pub(crate) fn into_verifier(self) -> Result<Option<Verifier>, CacheError>;
}
```

### Internal read-core assembly (src/composition.rs, private)

```rust
async fn build_read_core(
    source: Source,
    cache: CacheConfig,
    auth: AuthConfig,
) -> Result<(ReadOperations, PGlite, Option<String>, SourceShutdown), CacheError>
```

Branches once on `Source`:
- **Replica arm** (from `Di::init`): `setup::preflight` → `PGlite::open_multi_process` → `Replica::start` → `scan_schema` → `VersionIndex::new` → `CdcBridge::start(&replica, versions)` → `LiveHub::start` → `QueryCache::new(cache.max_bytes)` → `auth.into_verifier()` → `ReadOperations::new(...)`. Returns `dsn = None`, `SourceShutdown::Replica{replica, cdc}`.
- **Primary arm** (from `attach_child` minus az-wire, plus `open_primary` body): open embedded primary via internal `open_primary_db(&PrimarySource) -> (PGlite, String)` → `scan_schema` → `VersionIndex::new` → `CdcBridge::primary(versions)` → `LiveHub::start` → `auth.into_verifier()` → `ReadOperations::primary(...)` → `PrimaryObserver::start(...)`. Returns `dsn = Some(dsn)`, `SourceShutdown::Primary{observer: Some(observer)}`.

Bindings consume the returned `ReadOperations` uniformly — branch once, assemble uniformly, reusing the existing dual constructors.

Feature re-gating required: `PrimaryObserver` and `CdcBridge::primary`/`publish` are currently `#[cfg(feature = "az-wire")]`; HTTP-on-primary needs them under `#[cfg(feature = "read")]`.

### `ReadOperations::health` (src/operations.rs)

```rust
pub struct HealthStatus {
    pub halted: bool,
    pub reason: Option<String>,
    pub watermark: Option<u64>,
}
impl ReadOperations {
    pub async fn health(&self) -> HealthStatus;
}
```

Replica: `{halted, reason, Some(watermark)}`; primary (no replica): `{false, None, None}`.

## File Blueprint

### Files to CREATE

| File | Purpose | Complexity | Phase |
|------|---------|------------|-------|
| `src/composition.rs` | All composition types + `build_read_core` + `open`/`wait`/`shutdown` | High | 1–5 |

### Files to MODIFY

| File | Modifications | Complexity | Phase |
|------|---------------|------------|-------|
| `src/lib.rs` | Add `mod composition;` + `pub use composition::{PgPaw, PgPawBuilder, Source, ReplicaSource, PrimarySource, UpstreamConfig, CacheConfig, HttpConfig, AzWireConfig}` + `pub use auth::AuthConfig`; later delete `run`/`run_until`, old re-exports; keep `recover_primary`, `open_shadow`/`ShadowHandle`, `CacheError`/`LifecycleErrorKind`, `PreparedRead`/`ReadOperations`, `init` | High | 1,6,7 |
| `src/auth.rs` | Add `AuthConfig`; `AuthOutcome::from_request` reads `req.app_data::<web::Data<ReadOperations>>()` instead of `Di::instance()` | Medium | 1,4 |
| `src/operations.rs` | Add `HealthStatus` + `ReadOperations::health()` | Low | 2 |
| `src/http/server.rs` | `bind()` → `bind_at(addr, cors_origin, data: web::Data<ReadOperations>)` registering `.app_data(data.clone())`; drop `Di` | Medium | 4 |
| `src/http/query.rs` | Handlers take `web::Data<ReadOperations>`; drop `&'static` lifetimes in `live_query`/`private_response`; drop `Di` | Medium | 4 |
| `src/http/health.rs` | `healthz(data)` calls `data.health()`; drop `Di` | Low | 4 |
| `src/primary.rs` | Delete `PrimaryConfig`/`EmbeddedVerifierConfig`/`PrimaryHandle`/`open_primary`/`run_primary`/`finish_primary`; keep `recover_primary` + recovery helpers; re-gate `PrimaryObserver` to `read`; add internal `open_primary_db(&PrimarySource)` | High | 2,7 |
| `src/cdc.rs` | Re-gate `CdcBridge::primary`/`publish` from `az-wire` to `read` | Low | 2 |
| `src/az_wire.rs` | `register_az_wire` → `pub(crate)`; rewrite inline tests using old primary API | Medium | 5,8 |
| `src/setup.rs` | Adapt `UpstreamConfig` import to new home; publication/slot from `ReplicaSource` | Low | 2 |
| `src/main.rs` | Rewrite option `.config()` glue → builder mapping; `run_cli` opens builder then races `wait()` against `shutdown_signal` (moved here); `init` unchanged; update clap tests | High | 6 |
| `src/tests/mod.rs` | Verify compiles after moves (pure unit tests) | Low | 8 |

### Files to DELETE

| File | Reason |
|------|--------|
| `src/di.rs` | `Di` + singleton + `ServerConfig` deleted; assembly relocates to `composition.rs`; `UpstreamConfig` field-block reborn there; `merge_verdicts` tests already duplicated in `operations.rs` — drop the `di.rs` copies |

### Files to REVIEW (downstream consumers)

| File | Reason | Risk |
|------|--------|------|
| `tests/primary.rs` | Rewrite primary/child tests against builder; `recover_primary` tests unchanged | High |
| `integration-tests/src/lib.rs` | `pgpaw::run(ServerConfig)` at 3 sites (122, 302, 327) — rewrite `Server::launch` harness to builder | High |
| `bench/src/main.rs` | `pgpaw::run(ServerConfig)` (lines 244/269) — rewrite `spawn_pgpaw` to builder | Medium |
| `tests/topology_benchmark.rs` | Raw `az_wire::Node`, unaffected — confirm no old-API import | Low |
| `integration-tests/tests/*.rs` | Consume the harness, green once `lib.rs` harness migrates | Low |

## Implementation Phases (each phase compiles + tests green)

1. **Introduce new config types** — `composition.rs` skeleton, `CacheConfig`/`HttpConfig`/`AzWireConfig`/`Source`/`ReplicaSource`/`PrimarySource`/`UpstreamConfig` + `AuthConfig` in `auth.rs`; old types untouched.
2. **Unified read-core assembly** — re-gate `CdcBridge::primary`/`PrimaryObserver` to `read`; extract `open_primary_db`; implement `build_read_core` + `SourceShutdown`; add `ReadOperations::health()`.
3. **Runtime handle (bindingless)** — `PgPaw`, `builder()`, `open()` zero-binding case, `primary_dsn()`, `shutdown()`, `wait()` = pending.
4. **HTTP binding via web::Data** — migrate `AuthOutcome` + all handlers + `bind_at`; `open()` starts HTTP; prove HTTP-over-primary with `/healthz` 200 test.
5. **az-wire binding** — `register_az_wire` private; `open()` starts each topology with rollback; `wait()` selects over all bindings.
6. **CLI remap** — `shutdown_signal` to `main.rs`; `serve`/`primary` map flags to builder; `init` unchanged; clap tests updated.
7. **Hard-cut delete** — remove `di.rs`, `run`/`run_until`, old primary API, orphaned imports; feature-matrix build green (`read`, `server`, `az-wire`, no-default).
8. **Test migration** — rewrite `tests/primary.rs`, `integration-tests/src/lib.rs`, `bench/src/main.rs`, `az_wire.rs` inline tests; add shutdown-ordering tests per source×binding combo.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| `web::Data` migration touches 9 `Di::instance()` sites | High | Med | Confined to Phase 4; compiler-driven; `error_status`/`error_response` mapping untouched |
| `healthz` over primary has no `Replica` | Med | Med | `ReadOperations::health()` source-agnostic; explicit primary+HTTP `/healthz` → 200 test |
| Unification merely relocates the branch | Med | Med | `build_read_core` branches once; bindings consume `ReadOperations` with zero source knowledge |
| `PrimaryObserver`/`CdcBridge::primary` feature re-gating breaks matrix | Med | Med | Phase 2 re-gates to `read`; verify build matrix (precedent: commit b3f3038) |
| Shutdown ordering wrong for a combo (HTTP-on-primary unprecedented) | Med | High | Canonical order encoded once in `PgPaw::shutdown()`; Phase 8 ordering test per combo; standardize `LifecycleErrorKind` mapping |
| Downstream crates (`bench`, `integration-tests`) hard-break | High | Med | In-scope migration Phase 8; construction-only change; run their suites before merge |
| `wait()` needs `&mut self` but dream snippet reads immutable | Low | Low | `let mut pgpaw` at call sites; idiomatic |
| Deleting `di.rs` orphans its tests | Low | Low | Equivalent `merge_verdicts` tests already in `operations.rs` |

## Open Questions (resolved in design.md)

- `wait(&mut self)` vs dream's immutable-looking `pgpaw.wait()` → accept `&mut`, callers bind `let mut`.
- `sslmode` placement → defaulted field on `UpstreamConfig` (preserves `setup.rs`).
- `shutdown_signal` home → moves to `main.rs`; the `az_wire.rs` `sigterm_completes_the_production_signal_wait` test moves with it.

## Confidence Assessment

- **Design completeness**: 88 — every flagged question resolved with concrete signatures and placement; all 9 `Di::instance()` sites, both assembly paths, and downstream consumers accounted for.
- **Risk assessment accuracy**: 85 — highest risks (feature matrix, shutdown ordering per combo) have concrete Phase-2/Phase-8 mitigations; HTTP-on-primary has no runtime precedent, mitigated by an explicit new test.
- **Implementation feasibility**: 90 — phased order keeps tree green (new types before old deletes; hard cut in Phase 7 after all internal refs migrate). Code bodies relocated, not invented: one new file, one new enum (`SourceShutdown`), one small struct (`HealthStatus`).
