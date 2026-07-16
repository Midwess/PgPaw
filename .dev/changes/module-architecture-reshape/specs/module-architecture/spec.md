# Delta for Module Architecture

## ADDED Requirements

### Requirement: Module Ownership Boundaries

The crate SHALL organize `src/` into six responsibility-scoped modules — `api`, `source`, `capability`, `binding`, `db`, `protocol` — plus a crate-root `error` module and a `main.rs` CLI adapter, with each module owning exactly one architectural responsibility.

#### Scenario: Each module owns its responsibility

- WHEN the source tree is inspected
- THEN `api/` contains only builder/runtime/config types, `source/` only source-assembly functions, `capability/` only read-side semantic units, `binding/` only HTTP/az-wire exposure, `db/` only low-level pglite setup/open/recovery/observer/shadow primitives, and `protocol/` only subject constants and payload structs
- AND `src/composition.rs` no longer exists

#### Scenario: Dependency direction stays a DAG

- WHEN inter-module `use crate::` edges are enumerated
- THEN they form: api→{source, capability, binding, db}, source→{api::config, capability, db}, binding→{capability, protocol}, capability→(leaf), db→{capability::cdc, api::config}, protocol→(leaf)
- AND the only edge from `db` into `capability` is the pre-existing `PrimaryObserver → CdcBridge` coupling (accepted, not to be inverted)

#### Scenario: CLI stays a thin adapter

- WHEN `src/main.rs` is inspected
- THEN it contains only clap option structs, flag→builder mapping, signal handling, and logging setup — no composition, assembly, or capability logic

### Requirement: Flat Public Re-Exports

The crate SHALL re-export every public API type at the crate root so consumers write `pgpaw::{PgPaw, PgSource, ...}`, and SHALL NOT expose internal modules publicly, with `protocol` as the sole nested public module.

#### Scenario: Public paths stay flat

- WHEN a downstream consumer imports a builder/runtime/config/error/read type
- THEN it resolves at `pgpaw::<Type>` exactly as before the reshape (modulo the intentional renames)
- AND `pgpaw::api`, `pgpaw::source`, `pgpaw::capability`, `pgpaw::binding`, `pgpaw::db` are not publicly reachable

#### Scenario: Protocol is the sole nested public module

- WHEN wire payloads or subject constants are imported
- THEN they resolve at `pgpaw::protocol::payload::<T>` and `pgpaw::protocol::subjects::<C>`
- AND the former `pgpaw::wire::*` path no longer exists

### Requirement: Type Renames

The public API SHALL rename `Source` to `PgSource` and `PrimarySource` to `EmbeddedPrimarySource`, preserving variant names (`Replica`/`Primary`), constructor names (`replica`/`primary`/`embedded`), and all field and method semantics. `ReplicaSource` and `ReadOperations` SHALL keep their names.

#### Scenario: Renamed types are the only source types

- WHEN a consumer constructs a source
- THEN `PgSource::replica(ReplicaSource{..})` and `PgSource::primary(EmbeddedPrimarySource{..})` are the paths
- AND the identifiers `Source` and `PrimarySource` no longer resolve anywhere in the public API

### Requirement: Behavior Preservation

The reshape SHALL NOT change runtime behavior: SQL handling, replication, trigger-backed CDC, auth/RLS semantics, cache semantics, HTTP responses, az-wire behavior, CLI behavior, feature-gate matrix, or logfmt `event=` names.

#### Scenario: Feature matrix unchanged

- WHEN the crate is built with `--no-default-features --features read`, default features, `--no-default-features --features az-wire`, and `--all-features`
- THEN every combination compiles with zero warnings and every pre-existing test passes unmodified except import-path and rename updates

#### Scenario: Log event names unchanged

- WHEN any code path emits a log line
- THEN its `event=` name and field values are byte-identical to pre-reshape output
- AND only the `module_path!()`-derived `target=` values change (documented as unavoidable)

#### Scenario: Recovery remains available without read feature

- WHEN the crate is built without the `read` feature
- THEN `recover_primary` and its support functions remain compiled and callable (ungated), exactly as before

#### Scenario: az-wire passthrough unchanged

- WHEN an az-wire binding is configured
- THEN PgPaw still accepts `az_wire::NodeBuilder` and `az_wire::TopologyConfig` verbatim and registers the same `read`/`cursor`/`live` subjects

## MODIFIED Requirements

(none — no behavior requirement changes; composition/cli/access-control/realtime specs remain valid with the renamed type tokens)

## REMOVED Requirements

(none)
