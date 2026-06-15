# Codebase Analysis: jwt-access-control

Generated: 2026-06-15
Scope: HS256 JWT authentication + per-request authorization, consuming `PGlite::query_as` + `Replica::security_version` from the path-dep `pglite-rs`.

## Project Context
- actix-web 4, tokio, `pglite-rs` (path dep, features `replica`+`multiple-process`), sqlparser 0.52, moka 0.12, serde/serde_json, thiserror 2. **No `jsonwebtoken` yet.**
- Single binary; process-global `Di` singleton (`tokio::sync::OnceCell`); handlers call `Di::instance()` directly — **no `web::Data`, no actix middleware today.**

## Request flow for `POST /query` today (traced)
1. `server.rs:11` routes `POST /query` → `query::query`.
2. `query.rs:25` `pub async fn query(params: web::Query<QueryParams>, body: web::Json<QueryBody>) -> HttpResponse` → `materialize(sql)` or `live_query(sql)`.
3. `query.rs:71` `materialize` → `Di::instance()`; `replica().is_halted()` guard (`:75`).
4. `classifier().classify(sql)?` (`:83`) → `CacheableQuery`.
5. `hash = format!("{:x}", query.fingerprint)`; `version = versions().version_of(&tables,&eq_filters).0` (`:84-85`); `key = "{hash}:{version}"` (`:86`).
6. `cache().get_or_compute(key, async { rows::query_json(di.db(), &snapshot_sql).await })` (`:89-93`).
7. `query.rs:31-34` → `303 SeeOther`, `Location: /q/{hash}/{version}`, `Cache-Control: no-store`.
8. `GET /q/{hash}/{version}` = `cursor` (`query.rs:56`) serves from `QueryCache` with `ETag` + `Cache-Control: public, max-age=31536000, immutable` (`:61-64`). **No auth check.**

## Exact hooks
1. **`http/server.rs:6-20`** — `HttpServer::new(|| App::new().route("/healthz",…).route("/query",…).route("/q/{hash}/{version}",…))`. No `wrap()`, no `web::Data`. → add a `FromRequest` extractor on `query` only (keep `/healthz` + `/q/` open).
2. **`http/query.rs`** — `query` (`:25`), `cursor` (`:56`), `materialize` (`:71`), `error_response` (`:97-106`). 303 at `:31-34`; snapshot headers at `:61-64`. PRIVATE branch slots into `materialize` after classify: public → existing 303 path; private → `rows::query_json_as(...)` returned **inline** (never cached, never redirected).
3. **`classify.rs`** — `CacheableQuery { fingerprint:u64, tables:Vec<String>, eq_filters:Vec<(String,String)>, sql:String }` (`:13-18`); `ReadClassifier { replicated:HashSet<String> }` (`:40`); `classify(&self, sql) -> Result<CacheableQuery,CacheError>` (`:51`, **sync**); tables extracted at `:120-133`. Security verdict is async/db-bound → lives in a new `Di::is_private(&tables)` method, not in sync `classify`.
4. **`di.rs`** — `Di` struct (`:36-47`): `db, replica, versions, cache, classifier, live, tables, bind_addr, cdc`. `init(config)` (`:50`), `instance()` (`:96`), accessors `db()/replica()/versions()/cache()/classifier()/...` (`:106-136`). `ServerConfig` (`:28-34`). → add `jwt` config + `security_cache: Arc<Mutex<(u64, HashMap<String,bool>)>>` + method `is_private(&self,&[String]) -> Result<bool,CacheError>`.
5. **`cache.rs`** — `CachedResult { etag, body }` (`:6`); `get` (`:27`); `get_or_compute<F>` (`:31-48`). Key = `"{fingerprint:x}:{version}"`; ETag == key. No invalidation API (moka weight-evicts). Private path bypasses entirely.
6. **`rows.rs`** — `wrap_json` (`:5`), `query_json(db, sql)` (`:9`, the only `db.query` call site). → add sibling `query_json_as(db, role, claims, sql)` calling `db.query_as`.
7. **`error.rs:3-21`** — `CacheError` enum (`Pglite(#[from])`, `Upstream`, `Parse`, `Rejected`, `Config`, `Cache`, `Halted`, `Io`). **No `ResponseError` impl** — status mapping lives only in `error_response()` (`query.rs:98-106`). → add `Unauthorized(String)`→401, `Forbidden(String)`→403; extend `error_response` match + `name()`.
8. **`version.rs:14-18,68`** — `VersionIndex`; `version_of(&tables,&eq_filters) -> Lsn`. Nothing reads `Replica::security_version()` today (new).
9. **`main.rs:29-124`** — `Options` clap `#[derive(Args)]`, all fields `#[arg(long, env="…", global=true, default_value=…)]`. `ServerConfig` at `di.rs:28-34`. → add `#[arg(long="jwt-secret", env="JWT_SECRET", global=true)] jwt_secret: Option<String>` (+ optional `jwt_role_claim` default `"role"`), thread to `ServerConfig`/`Di::init`.
10. **Cargo.toml** — `pglite-rs = { path = "../pglite-rs/crates/pglite", features=["replica","multiple-process"] }`. Confirmed importable: `PGlite::query_as(role, claims:Option<&str>, sql, params)` (`pglite-rs …/replica/mod.rs:1020`) + `Replica::security_version() -> Result<u64,Error>` (`:434`), both gated on `replica`. **Add `jsonwebtoken = "9"`.**

## Dependencies
- `pglite` path dep (query_as + security_version — present). `jsonwebtoken = "9"` (HS256 + exp validation) — **new**. `serde_json` (serialize claims → `request.jwt.claims`). actix `FromRequest`/`Extensions`.
- `pglite::Error` already `#[from]` → `CacheError::Pglite`.

## Conventions
- `?` everywhere; ONE `CacheError` enum (add variants, no new error type, no `ResponseError` impl — extend `error_response`).
- New shared state on `Di` = `Arc<Mutex<T>>` per field, `&self`-only methods, locks dropped inside.
- Free fns in `rows.rs` for execution (matches `query_json`); a new `src/auth.rs` module is justified (genuinely new domain).
- Config = clap `#[arg(long, env, global)]`, `Option<T>` for optional. No inline comments.

## Risks
| Risk | Mitigation |
|---|---|
| `/q/` cursor is public + unauthenticated → a private snapshot would leak | Private queries **never** 303 / never enter `QueryCache`; return JSON inline. Assert `query_json_as` never writes cache. |
| public/private misclassification | fail-closed: ambiguous/error → `is_private = true`. |
| `security_cache` race (read version then refresh) | lock `(version, map)` as a unit; re-check version inside the lock. |
| `query_as` role/permission errors surface as `CacheError::Pglite` | map `Error::Database{sqlstate}` `42501`/`42704`/`28000` → 403; else 500. |
| middleware on all routes breaks `/healthz` probes | extractor on `query` only, not blanket `wrap()`. |
| `?live=true` + private → could stream private rows (live is v2 public-only) | classify first; private + live → 403. |

Confidence: pattern 97 / architecture 98 — whole codebase read; query_as + security_version signatures confirmed in the path dep.
