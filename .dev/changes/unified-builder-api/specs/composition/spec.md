# Delta for Composition

## ADDED Requirements

### Requirement: Single Composition Entry Point

The system SHALL expose exactly one composition entry point, `PgPaw::builder()`, returning a `PgPawBuilder` that collects one source, cache config, auth config, an optional HTTP binding, and zero-or-more az-wire bindings, and whose `.open()` returns a `PgPaw` runtime handle.

#### Scenario: Replica source with HTTP binding opens

- WHEN `PgPaw::builder().source(Source::replica(..)).http(HttpConfig{..}).open()` is awaited
- THEN a `PgPaw` runtime is returned, the HTTP server is listening on `HttpConfig.addr`, and `primary_dsn()` returns `None`

#### Scenario: Primary source with no bindings opens

- WHEN `PgPaw::builder().source(Source::primary(..)).open()` is awaited
- THEN a `PgPaw` runtime is returned and `primary_dsn()` returns the embedded Postgres DSN

#### Scenario: Open without a source fails

- WHEN `.open()` is awaited on a builder with no `.source(..)` call
- THEN it returns `CacheError::Config` and no resources are opened

### Requirement: Exactly One Source Determines Behavior

The system SHALL accept exactly one `Source` per instance — `Source::replica(ReplicaSource)` or `Source::primary(PrimarySource)` — and the source alone SHALL determine replica-vs-primary behavior. There SHALL be no separate mode-switch API.

#### Scenario: Replica source assembly

- WHEN the builder opens with `Source::replica`
- THEN the read core is built via the replica path (upstream preflight, embedded pglite open, `Replica::start`, WAL-driven CDC)

#### Scenario: Primary source assembly

- WHEN the builder opens with `Source::primary`
- THEN the read core is built via the primary path (embedded writable Postgres, LISTEN/NOTIFY-driven CDC via `PrimaryObserver`)

### Requirement: Any Binding Over Any Source

The system SHALL support every binding over every source, including HTTP over an embedded primary. All bindings SHALL consume the same read core (`ReadOperations`) with no source-specific knowledge.

#### Scenario: HTTP health over primary source

- WHEN a `PgPaw` opened with `Source::primary` and `HttpConfig` receives `GET /healthz`
- THEN it responds 200 with `{status:"ok"}` and no replica watermark fields

#### Scenario: HTTP health over replica source

- WHEN a `PgPaw` opened with `Source::replica` and `HttpConfig` receives `GET /healthz`
- THEN it responds with the replica watermark, or 503 when replication is halted, preserving existing halted/watermark semantics

### Requirement: az-wire Passthrough Bindings

The system SHALL accept az-wire bindings as verbatim `az_wire::NodeBuilder` + `az_wire::TopologyConfig` pairs via repeatable `.az_wire(node, topology)` calls accumulating in order. PgPaw SHALL register its services (`read`, `cursor`, `live`) on each node and start each topology as given. PgPaw SHALL NOT define its own az-wire binding options.

#### Scenario: Repeated az-wire bindings accumulate

- WHEN `.az_wire(..)` is called twice before `.open()`
- THEN two topologies are started, each exposing the `read`, `cursor`, and `live` subjects over the same read core

#### Scenario: Topology start failure rolls back

- WHEN an az-wire topology fails to start during `.open()`
- THEN already-started bindings and the read core are torn down and `.open()` returns `CacheError::lifecycle(LifecycleErrorKind::Topology, ..)`

### Requirement: Instance-Based Runtime

The system SHALL NOT hold PgPaw state in a process-global singleton. Multiple `PgPaw` instances MAY coexist in one process. HTTP handlers and the `AuthOutcome` request extractor SHALL receive state via actix `web::Data`.

#### Scenario: Two instances coexist

- WHEN two `PgPaw` instances open with distinct data directories and distinct binding addresses
- THEN both serve requests concurrently without shared-state interference

#### Scenario: Second open of same data directory fails cleanly

- WHEN a second `PgPaw` opens against a data directory already held by a live instance
- THEN `.open()` returns an error identifying the busy data directory and the first instance is unaffected

### Requirement: Canonical Shutdown Ordering

On `shutdown()`, the system SHALL stop bindings first (HTTP, then az-wire topologies in registration order), then source internals (replica: `replica.stop()` + CDC stop; primary: observer shutdown), then close the database last — for every source×binding combination. Shutdown errors SHALL map to `CacheError::lifecycle(LifecycleErrorKind::Shutdown, ..)`.

#### Scenario: Full-stack shutdown ordering

- WHEN `shutdown()` is called on a primary-source instance with HTTP and az-wire bindings
- THEN HTTP stops, topologies shut down, the observer stops, the database closes, in that order, and the data directory is released

### Requirement: Wait Semantics

`PgPaw::wait()` SHALL await the first fatal error or clean completion among started bindings and return that result. With zero bindings, `wait()` SHALL pend until the instance is shut down. Signal handling SHALL live in the CLI, not the library.

#### Scenario: Binding failure surfaces through wait

- WHEN a started binding terminates with an error while `wait()` is pending
- THEN `wait()` returns that error

#### Scenario: Bindingless wait pends

- WHEN `wait()` is awaited on an instance with no bindings
- THEN it does not return until `shutdown()` is initiated

## MODIFIED Requirements

### Requirement: Read-Core Behavior Preserved Under New Construction

The read core's externally observable behavior — query classification, snapshot cache and version semantics, JWT authentication/authorization (per `jwt-access-control/specs/access-control/spec.md`), and live/SSE wire semantics including `txid` and `reset` events (per `tanstack-db-live-sync/specs/realtime/spec.md`) — SHALL be preserved unchanged. The only change is construction: the read core is now assembled by the builder from either source, instead of by `Di::init` (replica) or `PrimaryHandle::attach_child` (primary).

#### Scenario: Access-control semantics survive builder construction

- WHEN a private-table query with a valid JWT is executed against a builder-opened instance
- THEN authentication, role routing, and uncached private execution behave exactly as specified in the access-control spec

#### Scenario: Live wire semantics survive builder construction

- WHEN a live subscription is opened against a builder-opened instance (either source)
- THEN delta, `up-to-date{txid}`, and `reset` events behave exactly as specified in the realtime spec

## REMOVED Requirements

### Requirement: `run`/`run_until` HTTP server entrypoint

Reason: replaced by `PgPaw::builder()..http(..).open()` + `wait()`; the monolithic lifecycle function is superseded by the runtime handle.

### Requirement: `open_primary`/`run_primary`/`PrimaryHandle::attach_child` primary entrypoints

Reason: replaced by `Source::primary` + optional bindings; read core no longer built lazily on child attach.

### Requirement: Global `Di` singleton

Reason: replaced by instance-owned state delivered to handlers via `web::Data`; one-instance-per-process restriction removed.

### Requirement: `EmbeddedVerifierConfig` auth path

Reason: replaced by source/binding-independent `AuthConfig`; primary sources configure auth identically to replica sources.

### Requirement: Public `register_az_wire`

Reason: az-wire service registration becomes internal to the az-wire binding; consumers pass `NodeBuilder`+`TopologyConfig` to `.az_wire(..)` instead.
