# Proposal: Unified Builder API

**Status**: approved

## Summary

Replace PgPaw's three fragmented entrypoints (`run`/`run_until`, `open_primary`/`run_primary`, `register_az_wire`) with one composition API: `PgPaw::builder()` taking a single `Source` (replica | primary), optional bindings (HTTP, az-wire passthrough), and returning an instance-based `PgPaw` runtime. Old API is hard-cut; CLI reduces to flag→builder mapping.

## Motivation

The current API shape is fragile and complex:

- **Three disjoint entrypoints** with divergent lifecycles: `run_until` (replica server, 115-line monolith interleaving HTTP, az-wire, and shutdown ordering), `open_primary`/`run_primary` + `PrimaryHandle::attach_child` (primary, read core built lazily and only when az-wire is requested), `open_shadow` (ephemeral).
- **Global `Di` singleton** (`OnceCell` in `src/di.rs:16`): one PgPaw instance per process, HTTP handlers and `AuthOutcome` reach into `Di::instance()` from 9 call sites across 5 files — untestable in parallel, state hidden from callers.
- **Duplicated auth config**: `ServerConfig` JWT fields vs `EmbeddedVerifierConfig` — primary sources can only configure auth through an az-wire-gated struct.
- **PgPaw invents az-wire options** (`ServerConfig.az_wire_addr`/`az_wire_node`) instead of accepting `az_wire::NodeBuilder` + `az_wire::TopologyConfig` natively, so PgPaw lags every az-wire topology capability.
- **Capabilities are mode-locked**: HTTP works only over replica, az-wire child only over primary — despite the read core (`ReadOperations`, `LiveHub`, cache) being source-agnostic already.

Target model: `CLI flags → Source config → PgPaw builder → optional HTTP binding → optional az_wire::NodeBuilder + az_wire::TopologyConfig → PgPaw runtime`. The source determines replica vs primary; bindings determine how clients reach the same read/live capabilities. No separate mode-switch API.

## Scope

### In Scope

- `PgPaw::builder()` → `PgPawBuilder` (`.source`, `.cache`, `.auth`, `.http`, repeatable `.az_wire`) → `.open()` → `PgPaw` runtime (`wait()`, `shutdown()`, `primary_dsn()`)
- `Source::replica(ReplicaSource)` / `Source::primary(PrimarySource)` — single source per instance
- Unified `CacheConfig`, `AuthConfig` (covers jwt_secret | jwt_public_key | jwt_jwks_url + role_claim, replaces both `ServerConfig` JWT fields and `EmbeddedVerifierConfig`), optional `HttpConfig`
- `AzWireConfig { node: az_wire::NodeBuilder, topology: az_wire::TopologyConfig }` — verbatim passthrough, `Vec` accumulation; PgPaw registers its services then starts the given topology
- Any binding × any source: HTTP over embedded primary supported now (read core built uniformly from either source)
- Kill `Di` global singleton — instance-owned state via actix `web::Data`
- Hard delete: `run`, `run_until`, `ServerConfig`, `UpstreamConfig`, `open_primary`, `run_primary`, `PrimaryConfig`, `PrimaryHandle` (+ `attach_child`), `EmbeddedVerifierConfig`, public `register_az_wire`, `Di`
- CLI `serve` and `primary` subcommands remapped to builder calls; flags unchanged
- Migrate `tests/primary.rs` and `src/az_wire.rs` test module to the builder API

### Out of Scope

- CLI flags exposing HTTP-on-primary (`pgpaw primary --port ...`) — lib supports it, CLI defers
- `init` (upstream prepare), `recover_primary`, `open_shadow` — kept, unchanged
- New az-wire capabilities or wire protocol changes
- Feature-flag restructuring — `read` / `server` / `az-wire` layering preserved

## Affected Areas

| Area | Impact |
|------|--------|
| `src/lib.rs` | `run`/`run_until` deleted; new `PgPaw`/`PgPawBuilder` exported; re-exports updated |
| `src/di.rs` | `Di` singleton, `ServerConfig`, `UpstreamConfig` deleted; replica read-core construction absorbed into builder assembly |
| `src/primary.rs` | `open_primary`/`run_primary`/`PrimaryHandle`/`attach_child`/`EmbeddedVerifierConfig` deleted; primary open/observer logic reused by builder; `recover_primary` kept |
| `src/az_wire.rs` | `register_az_wire` goes private; body unchanged |
| `src/http/server.rs`, `src/http/query.rs`, `src/http/health.rs` | `Di::instance()` → `web::Data`; `healthz` gains source-agnostic behavior (primary has no `Replica`) |
| `src/auth.rs` | `AuthOutcome` `FromRequest` reads `web::Data` instead of `Di::instance()`; new `AuthConfig` fluent constructors |
| `src/main.rs` | `ServeOptions::config()`/`PrimaryOptions::config()` glue rewritten as flag→builder mapping |
| `tests/primary.rs` | Rewritten against builder API (`recover_primary` tests stay) |
| `src/tests/`, `src/az_wire.rs` tests | Migrated to builder API |
| `tests/topology_benchmark.rs` | Unaffected (uses raw `az_wire::Node`) |
| `integration-tests/src/lib.rs` | `pgpaw::run(ServerConfig)` at 3 sites — `Server::launch` harness rewritten to builder |
| `bench/src/main.rs` | `pgpaw::run(ServerConfig)` at 2 sites — `spawn_pgpaw` rewritten to builder |

## Dependencies

- None external. `pglite-rs` and `az-wire` path deps unchanged; no new crates.
- Internal prerequisite: none — `LiveHub`, `QueryCache`, `VersionIndex`, `scan_schema` are already source-agnostic; `ReadOperations`/`CdcBridge` dual constructors are reused as-is with the builder as the single branch point.

## Risks

| Risk | Mitigation |
|------|------------|
| Singleton removal blast radius (9 `Di::instance()` call sites incl. `FromRequest`) | Mechanical `web::Data<T>` migration; compiler drives completeness; phase ordered so HTTP migration is isolated |
| `healthz` over primary source has no `Replica` (halted/watermark semantics undefined) | Design decision in design.md: source-agnostic health from read core, replica-specific fields conditional |
| Shutdown ordering must be correct for all source×binding combos (HTTP-on-primary has no precedent) | One canonical ordering: stop bindings (HTTP → topologies) → cdc/observer stop → replica.stop (replica only) → db close; generalizes both existing paths |
| Hard cut breaks downstream users on next release | Pre-1.0 crate; version bump signals break; CLI flags unchanged so operational surface is stable |
| Near-total rewrite of `tests/primary.rs` risks losing coverage | Map each existing test to a builder-based equivalent before deleting; scenarios in delta specs define required coverage |
| Error-mapping inconsistency (`CacheError::Config` vs `CacheError::lifecycle(Topology, ...)`) | Standardize on `CacheError::lifecycle` for all binding start/stop errors |
