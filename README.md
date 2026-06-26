# PgPaw

Read-only Postgres cache and realtime server over an embedded pglite-rs logical
replica.

PgPaw accepts read-only PostgreSQL `SELECT` queries, runs them against a local
pglite-rs replica, and returns either a cacheable snapshot URL or a live SSE
stream.

Full docs: https://midwess.com/pgpaw

## Install

```bash
cargo install pgpaw
```

or:

```bash
npm install -g pgpaw
```

## How it works

```txt
Upstream Postgres
  publication + logical replication slot
        |
        v
pglite-rs replica
  embedded multi-process Postgres
        |
        v
PgPaw
  ReadClassifier + VersionIndex + QueryCache + LiveHub
        |
        v
HTTP clients
  snapshots or SSE deltas
```

## Quickstart

Prepare upstream Postgres:

```bash
pgpaw init \
  --pg-host 127.0.0.1 \
  --pg-port 5432 \
  --pg-user postgres \
  --pg-password "$POSTGRES_PASSWORD" \
  --pg-database myapp
```

Start the server:

```bash
pgpaw serve \
  --host 127.0.0.1 \
  --port 8080 \
  --data-dir ./cache-data \
  --pg-host 127.0.0.1 \
  --pg-user postgres \
  --pg-password "$POSTGRES_PASSWORD" \
  --pg-database myapp
```

Query a public snapshot:

```bash
curl -i -X POST http://127.0.0.1:8080/query \
  -H "content-type: application/json" \
  -d '{"sql":"select id, email from users where id = 7"}'
```

Public response:

```http
HTTP/1.1 303 See Other
Location: /q/{hash}/{version}
Cache-Control: no-store
```

Fetch the snapshot:

```bash
curl http://127.0.0.1:8080/q/{hash}/{version}
```

Subscribe live:

```bash
curl -N -X POST "http://127.0.0.1:8080/query?live=true" \
  -H "content-type: application/json" \
  -d '{"sql":"select id, status from orders"}'
```

## HTTP API

| Method | Path | Purpose |
| --- | --- | --- |
| `POST` | `/query` | Run a read-only SQL query. Public queries redirect to a snapshot. Private queries return inline JSON. |
| `POST` | `/query?live=true` | Stream initial snapshot plus row deltas. |
| `GET` | `/q/{hash}/{version}` | Fetch cached public snapshot. |
| `GET` | `/healthz` | Return replica status and watermark. |

Snapshot hits return:

```http
HTTP/1.1 200 OK
Content-Type: application/json
ETag: {hash}:{version}
Cache-Control: public, max-age=259200
```

## Live events

Public query first event:

```txt
data: {"type":"snapshot","url":"/q/{hash}/{version}","version":42}
```

Private query first event:

```txt
data: {"type":"snapshot","rows":[{"id":1,"title":"Ship"}],"version":42}
```

Delta events:

```txt
data: {"op":"insert","key":"7","row":{"id":7,"title":"New"},"txid":123}
data: {"op":"update","key":"7","row":{"id":7,"title":"Done"},"txid":124}
data: {"op":"delete","key":"7","txid":125}
data: {"op":"up-to-date","txid":125}
```

`up-to-date` is sent after each relevant commit round, even when no row changed.

## SQL rules

PgPaw accepts one read-only `SELECT` over replicated tables.

Rejected input includes:

- multiple statements;
- writes and DDL;
- `SELECT INTO`;
- `FOR UPDATE` and `FOR SHARE`;
- non-replicated tables;
- side-effecting functions such as `nextval` and advisory lock functions;
- volatile functions such as `now`, `random`, and `gen_random_uuid`.

## Authorization

Public tables use the shared cache path. A query is private if any touched table
has RLS enabled or lacks `PUBLIC SELECT`.

Private queries require:

```http
Authorization: Bearer <token>
```

Configure one JWT key source:

```bash
pgpaw serve --jwt-secret "$JWT_SECRET" --jwt-role-claim role
```

or:

```bash
pgpaw serve --jwt-public-key "$JWT_PUBLIC_KEY" --jwt-role-claim role
```

`--jwt-jwks-url` is present as a CLI option, but JWKS verification is not
implemented yet.

## License

MIT
