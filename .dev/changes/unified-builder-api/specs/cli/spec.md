# Delta for CLI

## ADDED Requirements

### Requirement: CLI Only Maps Flags To The Builder

The CLI SHALL contain no composition logic of its own: each subcommand SHALL translate its flags into exactly one `PgPaw::builder()` call chain and delegate lifecycle to `open()`/`wait()`/`shutdown()`. Signal handling (ctrl-c / SIGTERM) SHALL live in the CLI, racing `wait()` and triggering `shutdown()`.

#### Scenario: Signal triggers graceful shutdown

- WHEN a running `pgpaw serve` or `pgpaw primary` process receives SIGINT or SIGTERM
- THEN the CLI stops racing `wait()`, calls `shutdown()`, and exits cleanly after the canonical shutdown ordering completes

### Requirement: Serve Maps To Replica Builder

`pgpaw serve` (and the default no-subcommand invocation) SHALL map to `PgPaw::builder().source(Source::replica(ReplicaSource{..})).cache(..).auth(..).http(HttpConfig{..})`, adding `.unb(NodeBuilder::new(node).insecure_accept_declared_peer_identities(), TopologyConfig::host(HostConfig::new(addr)))` only when `--unb-port` is set. Existing flag names, env vars, and defaults SHALL be preserved.

#### Scenario: HTTP-only serve

- WHEN `pgpaw serve --host 127.0.0.1 --port 8080 --data-dir ./pgpaw-data --pg-host 127.0.0.1 --pg-port 5432 --pg-database app` runs
- THEN the builder opens with a replica source and an HTTP binding on `127.0.0.1:8080` and no unb binding

#### Scenario: Serve with unb host

- WHEN `pgpaw serve --unb-port 8788 --unb-host 127.0.0.1 --unb-node pgpaw ..` runs
- THEN the builder additionally starts one unb host topology on `127.0.0.1:8788` with node identity `pgpaw`

### Requirement: Primary Maps To Primary Builder

`pgpaw primary` SHALL map to `PgPaw::builder().source(Source::primary(PrimarySource{..}))`, adding `.unb(NodeBuilder::new(node).insecure_accept_declared_peer_identities(), TopologyConfig::parent(ParentLink::unix(parent_node, parent_unix)))` only when the unb parent flags are set. No HTTP flags are exposed for `primary` in this change.

#### Scenario: Standalone primary

- WHEN `pgpaw primary --data-dir ./primary-data --database app --primary-listen 127.0.0.1 --primary-port 5432` runs
- THEN the builder opens with a primary source only, logs the DSN, and stays alive until a shutdown signal

#### Scenario: Primary as unb child

- WHEN `pgpaw primary .. --unb-node pgpaw --unb-parent-node worldant --unb-parent-unix /tmp/worldant.sock` runs
- THEN the builder additionally starts one unb parent-link topology connecting to `worldant` over the unix socket

## MODIFIED Requirements

### Requirement: Init Unchanged

`pgpaw init` SHALL keep its current flags and behavior verbatim: it prepares upstream Postgres (publication, slot prerequisites) via the library's `init(UpstreamConfig)` and performs no builder composition.

#### Scenario: Init prepares upstream

- WHEN `pgpaw init --pg-host H --pg-port P --pg-database D` runs
- THEN upstream preparation executes exactly as before this change

## REMOVED Requirements

### Requirement: CLI constructs `ServerConfig`/`PrimaryConfig`

Reason: those config types are deleted; the CLI's `.config()` conversion methods are replaced by builder mapping.
