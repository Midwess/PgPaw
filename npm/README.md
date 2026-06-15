<img src="https://raw.githubusercontent.com/Midwess/PgPaw/main/assets/avatar.png" alt="PgPaw" width="300" height="300" align="right" />

# PgPaw

> Read-only Postgres cache + realtime server over an embedded
> [pglite](https://crates.io/crates/pglite-rs) logical replica.

[![Crates.io](https://img.shields.io/crates/v/pgpaw)](https://crates.io/crates/pgpaw)
[![npm](https://img.shields.io/npm/v/pgpaw)](https://www.npmjs.com/package/pgpaw)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/Midwess/PgPaw/blob/main/LICENSE)
[![MSRV 1.85](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://github.com/Midwess/PgPaw/blob/main/Cargo.toml)
[![Repo](https://img.shields.io/badge/repo-Midwess%2FPgPaw-blue)](https://github.com/Midwess/PgPaw)

**PgPaw is what you get when [ElectricSQL](https://electric-sql.com) and
[Rocicorp Zero](https://zero.rocicorp.dev) meet on top of real Postgres.** One
endpoint serves **CDN-cacheable, immutable query snapshots** *and* **live
realtime updates** — yet, unlike shape- or ZQL-based sync engines, you query
with **plain Postgres SQL**: no bespoke query language to learn. It drops
straight into [TanStack](https://tanstack.com) Query / DB, and because every
query runs inside an embedded Postgres ([pglite](https://crates.io/crates/pglite-rs))
replica, you get **native Postgres Row-Level Security** — real RLS policies
enforced by Postgres itself, fully reliable.

PgPaw embeds a [pglite](https://crates.io/crates/pglite-rs) instance as a
**logical replica** of an upstream Postgres and serves raw read-only SQL over
HTTP, with three things that are usually painful to bolt on:

1. **Precise, watermark-derived cache invalidation** keyed on per-table / per-PK
   LSN state.
2. **CDN-cacheable, immutable versioned snapshots** — `303` redirects to a
   content-hash URL with `Cache-Control: public, max-age=31536000, immutable`.
3. **Realtime deltas over SSE** — first event is a snapshot pointer, then
   `insert` / `update` / `delete` events per affected row.

---

## Why

You have a Postgres you can't read-scalably, a CDN that wants immutable URLs,
and clients that want both a *snapshot* and a *live feed* from the same query.
PgPaw is a small single-purpose Rust service that sits in front and gives you
all three.

| If you want…                                  | You get…                                            |
| --------------------------------------------- | --------------------------------------------------- |
| Sub-millisecond cache hits in front of PG     | In-process `moka` cache, keyed on `(sql, version)`  |
| CDN cacheable responses                       | `303` → `/q/{hash}/{version}` with long max-age     |
| Live updates without re-querying              | SSE stream of row-level deltas from a CDC bridge    |
| Safe, read-only surface area                  | SQL-classifier rejects writes, DDL, `FOR UPDATE`…   |
| Tiny ops footprint                            | One static binary, one config, one upstream         |

---

## How it works

```
            ┌──────────────────────────┐
            │  Upstream Postgres       │
            │  (wal_level=logical,     │
            │   PUBLICATION,           │
            │   DDL event trigger)     │
            └────────────┬─────────────┘
                         │  logical replication
                         ▼
            ┌──────────────────────────┐
            │  pglite replica          │
            │  (embedded, multi-proc)  │
            └────────────┬─────────────┘
                         │
            ┌────────────┴─────────────┐
            │  PgPaw (this crate)      │
            │                          │
            │  ReadClassifier ──►      │ ──► POST /query ──► 303 ──► /q/{hash}/{v}
            │  VersionIndex (LSN) ──►  │       ▲                Cache-Control: immutable
            │  moka QueryCache ──►     │       │ live=true      (CDN-cacheable)
            │  CdcBridge (LSN feed) ─► │ ──► POST /query?live=true
            │  LiveHub (SSE deltas) ─► │       snapshot event
            └──────────────────────────┘      then insert/update/delete SSE
```

### Walkthrough

<img src="https://raw.githubusercontent.com/Midwess/PgPaw/main/assets/demo.svg" alt="PgPaw cache and live-delta walkthrough" width="880" />

The animation above is a 6-step loop. Watch the bottom panel:

1. **POST /query** — client posts a read-only `SELECT`. PgPaw parses, classifies,
   fingerprints the SQL.
2. **303 See Other** — instead of returning the body, PgPaw redirects to an
   **immutable** URL `/q/{hash}/v=42`. The `v=42` is the LSN-derived version
   for the touched `(table, pk, value)` tuple.
3. **CDN cache hit** — the client (or any CDN edge) serves `/q/9a4f/v=42` for
   the next year. The body never re-validates because the URL *is* the
   content+version.
4. **Upstream write** — an `UPDATE` on upstream Postgres commits, the logical
   replication feed carries it into the pglite replica, the `CdcBridge` thread
   updates `VersionIndex[(orders, id, 7)]` from 42 → 43.
5. **POST /query (same SQL)** — same fingerprint, but version is now 43, so
   PgPaw issues a **new** immutable URL `/q/9a4f/v=43`. The old URL keeps
   serving its frozen snapshot.
6. **SSE delta** — any subscriber to `/query?live=true` for that SQL receives
   the row-level `update` event, then an `up-to-date` heartbeat.

- **`ReadClassifier`** parses the SQL, rejects anything that isn't a single
  read-only `SELECT`, and extracts the table list + equality filters.
- **`VersionIndex`** consumes CDC transactions from the pglite replica and
  tracks the highest LSN per `(table, column, value)` (and per table for
  `TRUNCATE` / non-anchorable queries). The version of a query is the max LSN
  across the tables it touches.
- **`QueryCache`** stores `key = sql-fingerprint:version → result`. The
  fingerprint is stable for the parsed statement, the version is monotonic with
  upstream writes — so the cache key is immutable until a relevant write
  actually occurs.
- **`LiveHub`** re-runs the user SQL after each committed transaction that
  affects a touched table, diffs the new rows against the previous snapshot, and
  pushes row-level deltas to all subscribers.
- **Watermarks** are derived from CDC LSNs, not wall-clock, so cache hits and
  misses are deterministic with respect to upstream commit order.

---

## Install

PgPaw is a single static binary. Install it with whichever toolchain you have:

```bash
# via Cargo (builds from source)
cargo install pgpaw

# via npm (downloads the prebuilt binary for your platform)
npm install -g pgpaw
```

Prebuilt binaries: Linux x86_64 and macOS Apple Silicon. Other platforms build
from source via `cargo install pgpaw`.

---

## Quickstart

Prereqs: `pgpaw` installed (see [Install](#install)) and a reachable Postgres
13+ with superuser access (for the one-time `init` step).

```bash
# 1. one-time upstream prep: sets wal_level=logical, creates the
#    publication, and installs a DDL event trigger
pgpaw init \
    --pg-host 127.0.0.1 \
    --pg-user postgres \
    --pg-password $POSTGRES_PASSWORD \
    --pg-database myapp

# 2. restart Postgres if `init` changed wal_level (it prints a notice)

# 3. serve
pgpaw serve \
    --pg-host 127.0.0.1 \
    --pg-user postgres \
    --pg-password $POSTGRES_PASSWORD \
    --pg-database myapp \
    --port 8080
```

Try it:

```bash
# snapshot (HTTP cacheable)
curl -i -X POST http://127.0.0.1:8080/query \
     -H 'content-type: application/json' \
     -d '{"sql":"select id, email from users where id = 7"}'

# follow the 303
curl http://127.0.0.1:8080/q/<hash>/<version>

# live
curl -N -X POST 'http://127.0.0.1:8080/query?live=true' \
     -H 'content-type: application/json' \
     -d '{"sql":"select id, status from orders"}'
```

---

## HTTP API

| Method | Path                       | Purpose                                                     |
| ------ | -------------------------- | ----------------------------------------------------------- |
| `POST` | `/query`                   | Run a read-only `SELECT`, return a `303` to the snapshot    |
| `POST` | `/query?live=true`         | Run, then stream SSE deltas                                 |
| `GET`  | `/q/{hash}/{version}`      | Fetch an immutable, CDN-cacheable snapshot                  |
| `GET`  | `/healthz`                 | Liveness + replica watermark (`ok` / `halted`)              |

### `POST /query`

Request:

```json
{ "sql": "select id, email from users where id = 7" }
```

Response (snapshot mode): `303 See Other` with headers:

```
Location: /q/{hash}/{version}
Cache-Control: no-store
```

The hash is a `DefaultHasher` of the parsed statement, the version is the
`VersionIndex` value at lookup time.

### `GET /q/{hash}/{version}`

```
HTTP/1.1 200 OK
Content-Type: application/json
ETag: {hash}:{version}
Cache-Control: public, max-age=31536000, immutable
```

The body is the JSON-array result of the query, exactly as pglite returned it.
Safe to cache at the CDN edge for a year — the key is content-addressed and
reissued when upstream data changes.

### `POST /query?live=true`

Opens an SSE stream. First event:

```
data: {"type":"snapshot","url":"/q/{hash}/{version}","version":42}

```

Subsequent events are row deltas:

```
data: {"op":"insert","key":"7","row":{"id":7,"status":"paid"}}
data: {"op":"update","key":"7","row":{"id":7,"status":"refunded"}}
data: {"op":"delete","key":"7"}
data: {"op":"up-to-date"}
```

The terminal `up-to-date` event marks the end of one round of diffs; the
stream stays open and emits another batch on the next relevant commit.

### `GET /healthz`

```json
{ "status": "ok", "watermark": 12345678 }
```

or, when the replica has halted:

```json
{ "status": "halted", "reason": "..." }
```

### Error envelope

```json
{ "name": "RejectedError", "message": "table `secrets` is not available in this cache (not replicated)" }
```

Status codes: `400` for parse / rejection, `503` if the replica is halted,
`500` otherwise.

---

## SQL classifier — what's allowed

PgPaw only serves single-statement, read-only `SELECT` queries, and only
against tables present in the publication. Specifically it rejects:

- multi-statement input (`select 1; select 2`)
- `INSERT` / `UPDATE` / `DELETE` / `MERGE` / DDL
- `SELECT ... FOR UPDATE / FOR SHARE` (locking reads)
- `SELECT INTO` (writes a variable)
- side-effecting functions: `nextval`, `setval`,
  `pg_advisory_lock[_xact|_try|_unlock]`
- non-deterministic reads: `now`, `random`, `random_normal`, `clock_timestamp`,
  `timeofday`, `statement_timestamp`, `gen_random_uuid`, `uuid_generate_v4`
- references to tables not in the publication

A `WHERE col = literal` on a single table that is either the table's primary
key or a column on a `REPLICA IDENTITY FULL` table becomes a per-row anchor —
changes to other rows do not invalidate the cache entry.

---

## Configuration

All flags have matching environment variables.

### HTTP

| Flag      | Env            | Default       | Description                  |
| --------- | -------------- | ------------- | ---------------------------- |
| `--host`  | `CACHE_HOST`   | `127.0.0.1`   | bind host                    |
| `--port`  | `CACHE_PORT`   | `8080`        | bind port                    |
| `--data-dir` | `CACHE_DATA_DIR` | `./cache-data` | pglite replica data directory |
| `--max-connections` | `CACHE_MAX_CONNECTIONS` | `8`  | replica pool size            |
| `--cache-size-bytes` | `CACHE_SIZE_BYTES` | `268_435_456` (256 MiB) | result-cache budget |

### Upstream Postgres

| Flag             | Env                  | Default            | Description                          |
| ---------------- | -------------------- | ------------------ | ------------------------------------ |
| `--pg-host`      | `UPSTREAM_HOST`      | `127.0.0.1`        |                                      |
| `--pg-port`      | `UPSTREAM_PORT`      | `5432`             |                                      |
| `--pg-user`      | `UPSTREAM_USER`      | `postgres`         |                                      |
| `--pg-password`  | `UPSTREAM_PASSWORD`  | *(empty)*          |                                      |
| `--pg-database`  | `UPSTREAM_DATABASE`  | `postgres`         |                                      |
| `--publication`  | `UPSTREAM_PUBLICATION` | `cache_server_pub` | logical-replication publication    |
| `--slot`         | `UPSTREAM_SLOT`      | `cache_server_slot` | replication slot name               |
| `--sslmode`      | `UPSTREAM_SSLMODE`   | `disable`          | `disable` / `prefer` / `require` / `verify-full` |

## Testing

```bash
cargo test
```

The unit tests in `src/tests/mod.rs` cover:

- `ReadClassifier`: read-only `SELECT` acceptance, rejection of writes / DDL,
  `FOR UPDATE`, non-replicated tables, multi-statement, volatile functions,
  equality-filter extraction.
- `VersionIndex`: per-PK cache anchoring, per-value anchoring on
  `REPLICA IDENTITY FULL` tables, table-level fallback, and join-query
  fallback.
- `diff`: insert / update / delete detection, unchanged-row elision,
  `keyed_map` PK + hash-key fallback.

---

## License

[MIT](https://github.com/Midwess/PgPaw/blob/main/LICENSE) — see the file for full text.

## Acknowledgments

PgPaw is a focused, minimal extraction of the cache + realtime layer that
shipped inside the [pglite-rs](https://github.com/Midwess/pglite-rs) project.
The full commit history of that subsystem is preserved in this repository.
