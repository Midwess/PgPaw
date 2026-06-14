# PgPaw

Read-only Postgres cache + realtime server over an embedded
[pglite](https://crates.io/crates/pglite-rs) logical replica.

PgPaw embeds a pglite instance as a logical replica of an upstream Postgres and
serves raw read-only SQL over HTTP with precise, watermark-derived cache
invalidation, CDN-cacheable versioned snapshots, and realtime deltas.

Extracted from [pglite-rs](https://github.com/Midwess/pglite-rs); full commit
history preserved.

## Endpoints

- `POST /query` — body `{"sql": "..."}`. Returns `303` → `/q/{hash}/{version}`
  (immutable, CDN-cacheable snapshot of the result).
- `POST /query?live=true` — SSE: first event is a `{snapshot, url, version}`
  pointer to the cacheable snapshot, then row-level `insert`/`update`/`delete`
  deltas.
- `GET /q/{hash}/{version}` — the immutable result snapshot.
- `GET /healthz`.

## Run

```bash
# one-time: prepare the upstream (wal_level=logical, publication, DDL trigger)
cargo run -- init --pg-host 127.0.0.1 --pg-user <u> --pg-password <p> --pg-database <db>
# then restart Postgres if init changed wal_level, and serve:
cargo run -- serve --pg-host 127.0.0.1 --pg-user <u> --pg-password <p> --pg-database <db> --port 8080
```
