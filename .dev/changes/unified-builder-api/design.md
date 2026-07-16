# Design: unified-builder-api

## Overview

One composition pipeline replaces three entrypoints:

```
CLI flags
  -> Source config (ReplicaSource | PrimarySource)
  -> PgPaw builder (.cache, .auth)
  -> optional HTTP binding (HttpConfig)
  -> optional az_wire::NodeBuilder + az_wire::TopologyConfig (repeatable)
  -> PgPaw runtime (wait / shutdown / primary_dsn)
```

`open()` builds the read core once (branching exactly once on `Source`), wraps it in `web::Data`, starts every requested binding, and returns an instance-owned runtime. No global state.

## Architecture

```
PgPawBuilder::open()
  ├── build_read_core(source, cache, auth)          [composition.rs, private]
  │     ├── Replica arm  (ex Di::init):    preflight → PGlite::open_multi_process → Replica::start
  │     │                                  → scan_schema → VersionIndex → CdcBridge::start → LiveHub
  │     │                                  → QueryCache → Verifier → ReadOperations::new
  │     └── Primary arm  (ex attach_child): open_primary_db → scan_schema → VersionIndex
  │                                          → CdcBridge::primary → LiveHub → Verifier
  │                                          → ReadOperations::primary → PrimaryObserver::start
  │     → (ReadOperations, PGlite, Option<dsn>, SourceShutdown)
  ├── HTTP binding:   bind_at(addr, cors, web::Data<ReadOperations>)     [if .http()]
  └── az-wire binding: register_az_wire(node, read.clone()).build()?
                         .start_topology(topology)                        [per .az_wire()]
```

Bindings never see the `Source` — they consume `ReadOperations` uniformly. This is what makes HTTP-over-primary fall out for free.

## Key Decisions

### Decision 1: Kill the `Di` singleton, state via `web::Data`

**Context:** `Di` is a `'static OnceCell` with 9 `Di::instance()` call sites across `auth.rs`, `http/*`, `lib.rs`; it caps the process at one PgPaw instance and hides state flow.
**Options:**
1. Keep singleton behind the builder facade — small diff, but fragility and one-instance limit remain; tests still serialize.
2. Instance-based state via actix `web::Data<ReadOperations>` — bigger diff (handler signatures + `AuthOutcome::from_request` via `req.app_data`), multiple instances per process, state flow explicit.
**Decision:** Option 2. The fragility complaint is the motivation for this change; a facade over a global would preserve it. `HttpServer::new` closure already captures config by move (`cors_origin`), so `web::Data` injection follows the existing pattern.

### Decision 2: Read core built uniformly — branch once on `Source`

**Context:** Replica core is built eagerly in `Di::init`; primary core lazily in `attach_child` and only when az-wire is requested. Decision locked: any binding over any source.
**Options:**
1. Merge `ReadOperations::new`/`::primary` and `CdcBridge::start`/`::primary` into single constructors — invasive, churns unrelated internals.
2. Keep dual constructors; `build_read_core` is the single call site branching once on `Source`, both arms converging on `ReadOperations`.
**Decision:** Option 2 (Least New Definitions: reuse existing constructors). Requires re-gating `PrimaryObserver` and `CdcBridge::primary`/`publish` from `az-wire` to `read` — primary CDC is now needed by HTTP too.

### Decision 3: Source-specific runtime state = `SourceShutdown` enum

**Context:** Runtime must own replica teardown handles (`Replica`, `CdcBridge`) or primary teardown handles (`PrimaryObserver`) — mutually exclusive.
**Options:**
1. Flat `Option` fields for every source-specific handle — invalid states representable (replica + observer both `Some`).
2. `enum SourceShutdown { Replica{replica, cdc}, Primary{observer} }` carrying per-source teardown behavior.
**Decision:** Option 2 — the one genuinely new internal type; makes invalid combinations unrepresentable and hosts the per-source teardown ordering. Shared fields (`db`, `read`, `dsn`, bindings) stay flat on `PgPaw` (No Fake Inner Structs).

### Decision 4: `healthz` goes source-agnostic via `ReadOperations::health()`

**Context:** `http/health.rs` calls `Di::instance().replica()` for halted/watermark checks; primary sources have no `Replica`.
**Options:**
1. Branch inside the handler per source — handler learns about sources, violates binding uniformity.
2. `ReadOperations::health() -> HealthStatus {halted, reason, watermark: Option}` — replica populates watermark/halt from its `Option<Replica>`; primary reports ok.
**Decision:** Option 2. `ReadOperations` already holds `Option<Replica>` and is the facade every binding consumes. Replica health semantics preserved verbatim; primary returns `{status:"ok"}`.

### Decision 5: Canonical shutdown ordering

**Context:** Two divergent orderings exist (server: HTTP→topology→replica/cdc/db; primary: topology→observer→db.close); HTTP-on-primary has no precedent.
**Decision:** One ordering in `PgPaw::shutdown()`: HTTP stop → az-wire topologies in registration order → `SourceShutdown` teardown (replica.stop + cdc.stop, or observer.shutdown) → db close last. All binding lifecycle errors standardized on `CacheError::lifecycle(LifecycleErrorKind::{Topology,Shutdown}, _)` (the old paths inconsistently used `CacheError::Config`).

### Decision 6: `wait()` semantics and signals

**Context:** `run()` embedded signal handling; embedded primary use (dsn-only, zero bindings) must stay alive without spinning.
**Decision:** `wait(&mut self)` selects biased over all binding futures, returns first fatal error or clean completion; zero bindings → pends until shutdown. Signal handling moves entirely to `main.rs` (CLI races `wait()` against `shutdown_signal`, then calls `shutdown()`). `wait` takes `&mut self` because actix `Server` and `AzWireTopology::wait` poll by `&mut` — callers bind `let mut pgpaw`.

### Decision 7: Placement and open-question resolutions

- New types live in one new file `src/composition.rs` (genuinely new domain: composition); `AuthConfig` in `src/auth.rs` next to `Verifier`; `HealthStatus` in `src/operations.rs`. Net files: +`composition.rs`, −`di.rs`.
- `sslmode` stays a defaulted field on the relocated `UpstreamConfig` (`"disable"`), preserving `setup.rs` signatures; `publication`/`slot`/`max_connections` hoist to `ReplicaSource` per the dream API.
- `shutdown_signal` moves to `main.rs`; the `sigterm_completes_the_production_signal_wait` test moves with it.
- `AuthConfig` fluent constructors each set exactly one key source; validation (multi-key rejection) stays in the unchanged `Verifier::build` via `into_verifier()`.

## API Changes

Public surface after the change (crate root):

```rust
pub use composition::{PgPaw, PgPawBuilder, Source, ReplicaSource, PrimarySource,
                      UpstreamConfig, CacheConfig, HttpConfig, AzWireConfig};
pub use auth::AuthConfig;
pub use error::{CacheError, LifecycleErrorKind};
pub use operations::{PreparedRead, ReadOperations};
pub use primary::recover_primary;
pub use shadow::{open_shadow, ShadowHandle};
pub async fn init(upstream: UpstreamConfig) -> Result<(), CacheError>;   // unchanged
```

Deleted: `run`, `run_until`, `ServerConfig`, old `UpstreamConfig` home, `Di`, `open_primary`, `run_primary`, `PrimaryConfig`, `PrimaryHandle`, `EmbeddedVerifierConfig`, `register_az_wire`.

HTTP endpoints, wire protocol, and az-wire subjects are byte-for-byte unchanged; only construction changes.

## Security Considerations

- Auth unification widens capability, not surface: primary sources gain the full `AuthConfig` (public key, JWKS) previously replica-only; verification logic (`Verifier::build`) unchanged.
- `insecure_accept_declared_peer_identities()` remains an explicit caller/CLI choice passed through `NodeBuilder` — PgPaw never applies it implicitly in the library builder.
- Fail-closed classification and private-query semantics are untouched (covered by MODIFIED requirement referencing the access-control spec).
