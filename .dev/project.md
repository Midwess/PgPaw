# PgPaw — Project Context (dev-workflow)

## Overview
Read-only Postgres cache + realtime server over an embedded pglite logical replica. Serves raw read-only `SELECT` over HTTP with watermark-derived cache invalidation, CDN-cacheable immutable snapshots, and SSE row-level deltas.

## Tech stack
Rust 1.85, actix-web 4, tokio, `pglite-rs` (features `replica` + `multiple-process`; **currently a path dep on `../pglite-rs/crates/pglite`** for the in-progress access-control work — revert to a published version before release), sqlparser 0.52, moka 0.12, serde/serde_json, thiserror 2.

## Architecture (modules)
- `main.rs` / `setup.rs` — clap CLI (flag→builder mapping + signal handling) + upstream prepare/preflight.
- `composition.rs` — the composition domain: `PgPaw::builder()` → `PgPawBuilder` (`Source::replica`/`Source::primary`, `CacheConfig`, `AuthConfig`, optional `HttpConfig`, repeatable `.az_wire(NodeBuilder, TopologyConfig)`) → `open()` → instance-based `PgPaw` runtime (`wait`/`shutdown`/`primary_dsn`). `build_read_core` branches once on `Source`; bindings consume `ReadOperations` uniformly (HTTP-over-primary works). No global singleton — HTTP handlers get state via `web::Data<ReadOperations>`.
- `http/{server,query,health}.rs` — routes: `POST /query` (303 → snapshot), `POST /query?live=true` (SSE), `GET /q/{hash}/{version}`, `GET /healthz`.
- `classify.rs` — `ReadClassifier`: AST gate (sqlparser) — accepts a single read-only `SELECT` over published tables, extracts tables + equality filters, rejects writes/DDL/volatile/locking.
- `version.rs` — `VersionIndex`: per `(table,col,value)` / per-table LSN watermark; a query's version = max over its anchors.
- `cache.rs` — `QueryCache` (moka), key = `(sql-fingerprint, version)`.
- `cdc.rs` — `CdcBridge`: drains `Replica::subscribe()`, advances `VersionIndex`, fans out to `LiveHub`.
- `live.rs` / `diff.rs` — SSE delta hub + row diffing.
- `rows.rs` — query execution → JSON.
- `error.rs` — unified thiserror `CacheError`.

## Conventions (from CLAUDE.md)
- `?` everywhere; ONE unified thiserror enum (`CacheError`).
- Least New Definitions (precedes Struct-First); attach to existing structs; reuse > create.
- No inline comments. Simplicity First. Surgical changes.
- Locks hidden behind `&self`; interior mutability; `#[derive(Clone)]` with per-field `Arc`, no `Inner` structs.

## Current work
Access-control parity (RLP). The `pglite-rs` replica now replicates the upstream security catalog (roles / table grants / RLS flags / policies) and exposes `PGlite::query_as(role, claims, sql, params)` + `Replica::security_version()`. PgPaw's half (this domain): **JWT authentication (HS256) + per-request authorization** — verify the bearer token (401 on missing/invalid), classify a query as public vs access-controlled, route public → existing global cache, access-controlled → `query_as(role, claims)` live and uncached. See `.dev/changes/`.

## Latest Analysis

Last updated: 2026-07-16 — change `module-architecture-reshape` (proposal; follows `unified-builder-api`)

### Architecture Summary (module-architecture-reshape)
Pure structural proposal: flat `src/` → six ownership-scoped modules (`api/`, `source/`, `capability/`, `binding/`, `db/`, `protocol/`), `composition.rs` dissolved, `Source`→`PgSource`, `PrimarySource`→`EmbeddedPrimarySource`, `pgpaw::wire`→`pgpaw::protocol::{payload,subjects}`. Flat crate-root re-exports preserved. Zero behavior change; 4-combo feature matrix per phase is the gate. Full item-by-item move map + cfg-gate redistribution tables in `.dev/changes/module-architecture-reshape/analysis.md` (§1–2) — the two dense splits are `composition.rs` (8 gate rules) and `operations.rs` (`is_private` complementary cfg arms).

### Prior analysis (2026-07-16 — unified-builder-api)

### Architecture Summary (2026-07-16)
Two independent "read core" construction paths exist today — `Di::init` (replica, eager, stored in a global `OnceCell<Di>`) and `PrimaryHandle::attach_child` (primary, deferred, only triggered by az-wire) — both assembling the same components (`VersionIndex`, `CdcBridge`, `LiveHub`, `QueryCache`, `Verifier` → `ReadOperations`) via different constructors (`ReadOperations::new` vs `::primary`, `CdcBridge::start` vs `::primary`). HTTP handlers (`src/http/*`) and `AuthOutcome` (`src/auth.rs`) reach into `Di::instance()` directly — no `web::Data`. 9 `Di::instance()` call sites across 5 files.

### Key Patterns (2026-07-16)
- Dual constructors per source (`ReadOperations::new`/`primary`, `CdcBridge::start`/`primary`) — the precedent for reconciling `Source::replica`/`Source::primary` into one read-core assembly step.
- `run_until` (src/lib.rs) already implements the shutdown ordering (bindings → topology → core) the new `PgPaw::shutdown()` must generalize.
- `register_az_wire(builder, operations) -> NodeBuilder` is builder-pattern-ready; keep body, drop `pub` export.
- Lifecycle errors: prefer `CacheError::lifecycle(LifecycleErrorKind::X, source)` over new `CacheError` variants.
- Feature layering `az-wire ⊃ read`, `server ⊃ read` must be preserved by the builder surface.

### Prior analysis (2026-06-15 — jwt-access-control)

### Architecture Summary
Single-process actix-web 4 server; process-global `Di` singleton (`tokio::sync::OnceCell`) accessed via `Di::instance()` inside handlers — **no `web::Data`, no actix middleware today**. Request path: `query` → `materialize` → `classifier().classify` → `versions().version_of` → `cache().get_or_compute(rows::query_json)` → `303 /q/{hash}/{version}` (snapshot served by `cursor` with `public, max-age, immutable` + `ETag`). HTTP status mapping lives only in `error_response()` (`query.rs`), not a `ResponseError` impl.

### Key Patterns
- `Di::instance()` global singleton; new shared state = `Arc<Mutex<T>>` field, `&self` methods.
- Execution layer = free fns in `rows.rs` (`query_json`); cache key = `"{fingerprint:x}:{lsn.0}"`, ETag == key.
- Errors = one `CacheError` enum; add variants + map in `error_response` (no `ResponseError`).
- Config = clap `#[arg(long, env, global)]`, `Option<T>` for optional.

### jwt-access-control hooks
- Auth = new `src/auth.rs` (`Principal` + HS256 verify + `OptionalPrincipal` `FromRequest` on `query` only).
- Verdict = `Di::is_private(&tables)` over the replicated catalog (`relrowsecurity` + `has_table_privilege('public', oid, 'SELECT')`), cached + invalidated by `Replica::security_version()`, fail-closed.
- Private path = `rows::query_json_as` → `db.query_as` returned **inline** (never `303`, never cached); public path unchanged except snapshot `max-age` → 72h.
- `pglite-rs` is a **path dep** for `query_as`/`security_version` (revert before release); `jsonwebtoken = "9"` new.

### tanstack-db-live-sync hooks (2026-06-16)
- Live wire gains `txid` (CDC `CommittedTransaction.xid`, u32) on deltas + `up-to-date`; `up-to-date{txid}` emitted even on empty diff so client `awaitTxId` can't hang.
- RLS live: `LiveHub::Subscription` holds `Option<Principal>`; `on_commit` recompute = `query_json_as(role, claims)` when private; `http/query.rs` drops the live `Forbidden` and passes the `Principal`; private first event = inline `rows` (no `/q` pointer).
- `reset` event on `RecvError::Lagged` → client truncates + reloads.
- New npm package `packages/tanstack-db` (`@pgpaw/tanstack-db`): native TanStack DB collection over the SSE wire. Backend stays TanStack-agnostic.

