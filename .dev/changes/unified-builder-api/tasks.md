# Tasks: unified-builder-api

## Progress: [34/34]

## 1. New Config Types (tree stays green, old API untouched)

- [x] 1.1 Create `src/composition.rs`, add `mod composition;` to `lib.rs`
- [x] 1.2 Define `CacheConfig` (Default = 256 MiB), `HttpConfig` (server-gated, `SocketAddr` + `cors_origin`), `UnbConfig` (unb-gated), `Source`/`ReplicaSource`/`PrimarySource` (+ `PrimarySource::embedded`, `Source::replica`/`primary`), relocated `UpstreamConfig` field-block (sslmode defaulted `"disable"`)
- [x] 1.3 Add `AuthConfig` + fluent constructors (`none`, `jwt_secret`, `jwt_public_key`, `jwt_jwks_url`, chainable `role_claim`) + `into_verifier()` to `src/auth.rs`; unit-test delegation to `Verifier::build`
- [x] 1.4 `pub use` new types from `lib.rs`; `cargo build --all-features` + existing tests green

## 2. Unified Read-Core Assembly

- [x] 2.1 Re-gate `CdcBridge::primary`/`publish` (src/cdc.rs) and `PrimaryObserver` (src/primary.rs) from `#[cfg(feature = "unb")]` to `#[cfg(feature = "read")]`
- [x] 2.2 Extract internal `open_primary_db(&PrimarySource) -> (PGlite, String)` in `src/primary.rs` (open + dsn derivation, reusing recovery helpers)
- [x] 2.3 Implement `build_read_core(source, cache, auth) -> (ReadOperations, PGlite, Option<String>, SourceShutdown)` in `composition.rs`, branching once on `Source` (replica arm from `Di::init` body, primary arm from `attach_child` body); define `SourceShutdown`
- [x] 2.4 Add `HealthStatus` + `ReadOperations::health()` to `src/operations.rs`; `#[serial]` integration test builds each source's core and asserts `prepare`/`health` work

## 3. Runtime Handle (bindingless)

- [x] 3.1 Implement `PgPaw` struct, `PgPaw::builder()`, `PgPawBuilder::open()` for the zero-binding case
- [x] 3.2 Implement `primary_dsn()`, `shutdown()` (source teardown via `SourceShutdown`, db last), `wait()` zero-binding = pending
- [x] 3.3 `open()` validation: missing source → `CacheError::Config`; test primary open → `primary_dsn().is_some()`, shutdown releases data dir

## 4. HTTP Binding via web::Data

- [x] 4.1 Migrate `AuthOutcome::from_request` (src/auth.rs) to `req.app_data::<web::Data<ReadOperations>>()`
- [x] 4.2 Migrate `query`/`cursor`/`healthz`/`private_response`/`live_query` handlers to `web::Data<ReadOperations>`; drop `&'static` lifetimes and `Di` imports
- [x] 4.3 Replace `bind()` with `bind_at(addr, cors_origin, data)` registering `.app_data(data.clone())` (src/http/server.rs)
- [x] 4.4 Wire HTTP into `open()`/`wait()`/`shutdown()`; test HTTP-over-replica via builder harness AND HTTP-over-primary `/healthz` → 200

## 5. unb Binding

- [x] 5.1 `register_unb` → `pub(crate)`; drop `pub use` from `lib.rs`
- [x] 5.2 `open()` starts each `UnbConfig` (`register_unb(node, read.clone()).build()?.start_topology(topology)`), errors mapped to `CacheError::lifecycle(Topology, _)`, rollback of started bindings + core on failure
- [x] 5.3 `wait()` selects biased over HTTP future + each topology `.wait()`; `shutdown()` stops topologies after HTTP, before source teardown; replica+unb and primary+unb builder tests

## 6. CLI Remap

- [x] 6.1 Move `shutdown_signal` from `lib.rs` to `main.rs` (move the `sigterm_completes_the_production_signal_wait` test from `unb.rs` along with it)
- [x] 6.2 `ServeOptions` maps to `Source::replica(ReplicaSource)` + `CacheConfig` + `AuthConfig` + `HttpConfig` + optional `.unb(NodeBuilder::new(node).insecure_accept_declared_peer_identities(), TopologyConfig::host(HostConfig::new(addr)))` when `--unb-port` set
- [x] 6.3 `PrimaryOptions` maps to `Source::primary(PrimarySource)` (no HTTP flags per scope)
- [x] 6.4 `run_cli`: `serve`/`primary` → `builder.open().await?`, race `wait()` against `shutdown_signal`, then `shutdown().await?`; `init` unchanged
- [x] 6.5 Update clap tests to assert builder/Source mapping instead of `ServerConfig`

## 7. Hard-Cut Delete of Old API

- [x] 7.1 Delete `src/di.rs`; remove `mod di;` + `pub use di::{Di, ServerConfig, UpstreamConfig}` from `lib.rs`
- [x] 7.2 Delete `run`/`run_until` from `lib.rs`
- [x] 7.3 Delete `PrimaryConfig`/`EmbeddedVerifierConfig`/`PrimaryHandle`/`open_primary`/`run_primary`/`finish_primary` from `src/primary.rs`; keep `recover_primary` + recovery helpers + `PrimaryObserver` + `open_primary_db`
- [x] 7.4 Remove orphaned imports; verify feature matrix builds: `--no-default-features --features read`, `--features server`, `--features unb`, `--all-features`

## 8. Test Migration

- [x] 8.1 Rewrite `tests/primary.rs` primary/child tests against builder (`recover_primary` tests unchanged)
- [x] 8.2 Rewrite `integration-tests/src/lib.rs` `Server::launch` + failure helper (3 `pgpaw::run` sites) to builder; endpoint tests unchanged
- [x] 8.3 Rewrite `bench/src/main.rs` `spawn_pgpaw` (2 `pgpaw::run` sites) to builder
- [x] 8.4 Rewrite `src/unb.rs` inline tests that used old primary API
- [x] 8.5 Add `wait()`/`shutdown()` ordering tests across combos: replica+http, replica+unb, primary+http, primary+unb, primary-only

## 9. Documentation

- [x] 9.1 Update README examples to builder API (serve/primary/unb snippets)
- [x] 9.2 Update `.dev/project.md` architecture section (di.rs removed, composition.rs added)

---

## Notes

- Each phase must compile with tests green before starting the next; old API is deleted only in Phase 7 after all internal references migrate.
- `tests/topology_benchmark.rs` uses raw `unb::Node` — confirm unaffected, no rewrite.
- Standardize all binding start/stop error mapping on `CacheError::lifecycle(LifecycleErrorKind::{Topology,Shutdown}, _)`.

### Implementation deviations & decisions (recorded during apply)

- **actix `Server` is a lazy future** — storing it unpolled meant HTTP never served until `wait()`. Fixed: `open()` spawns it (`tokio::spawn`), `PgPaw` holds `http_task: JoinHandle` + `ServerHandle`; bindings serve immediately after `open()`. HTTP binding requires a tokio (or actix System) runtime context at `open()`.
- `init` signature changed to `init(upstream: UpstreamConfig, publication: &str)` — publication hoisted out of `UpstreamConfig` per the dream API; CLI behavior unchanged.
- `wait()` takes `&mut self` (actix Server task + `UnbTopology::wait` poll by `&mut`); callers bind `let mut pgpaw`.
- Primary read core (triggers, classifier tables) snapshots schema at `open()` — tables created afterwards are not observed; embedded-child test bootstraps schema in a first bindingless open, then reopens with bindings. Same constraint existed at `attach_child` time before.
- Old `attach_child` "listenerless parent topology" validation dropped — topology passthrough is verbatim per the composition spec.
- Primary `QueryCache` now sized by `CacheConfig` (default 256 MiB) instead of hardcoded 64 MiB.
- CLI `primary` gains `--database` and `--unb-node`/`--unb-parent-node`/`--unb-parent-unix` flags (dream CLI shape); old flags unchanged.
- `PgPaw::live_subscription_count()` relocated from deleted `PrimaryHandle` (test observability).
- `HealthStatus` exported under `server`; primary `/healthz` = 200 `{status:"ok"}` without watermark.
- `src/http/server.rs` inline port-release test replaced by builder-level `http_bind_conflict_rolls_back_the_primary_source` (covers stop-ordering + data-dir release).
- Verified green: 48 lib tests, 9+1 bin tests, 9 `tests/primary.rs` tests, feature matrix (`none`/`read`/`server`/`unb`/`--all-features --all-targets --workspace`) zero warnings.

### Code review (2026-07-16, 4 core agents, threshold 80) — 3 findings, all fixed

1. **[85] `PgPaw::wait()` cancellation-unsafety** (history-analyzer, vs commit 3e212b4): polling `topology.wait()` futures take()s unb's parent-link `JoinHandle` at first poll; dropping `wait()` (signal winning `run_pgpaw`'s select) made a later `shutdown()` silently skip awaiting the child task. Fixed by mirroring 3e212b4: `wait()` polls sync `is_finished()` (10ms sleep loop, non-consuming) and calls `topology.wait()` only after completion, so errors still surface through `wait()` and `shutdown()` always finds an intact task. Regression test: `interrupted_wait_still_shuts_down_a_parent_linked_child`.
2. **[85] `PgPaw::shutdown()` leak on early `?`** (bug-detector): HTTP join error / topology shutdown error / observer unlisten failure returned before source teardown, leaking the embedded Postgres process + data-dir lock. Fixed: shutdown always completes full teardown (HTTP → topologies → source → db) and returns the first error at the end.
3. **[85] `open_primary_db` bootstrap leak** (bug-detector): `query`/`exec` failures in the ensure-database block skipped `bootstrap.close()`. Fixed with explicit close-on-error.

Clean: claude-md-auditor (8 guideline groups, 14 files), spec-validator (both delta specs valid). Post-fix verification: full suite + matrix green again.
