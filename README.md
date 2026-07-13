# PgPaw

PgPaw is a read-only HTTP and realtime layer for Postgres. It keeps a local
[pglite-rs](https://crates.io/crates/pglite-rs) logical replica of your upstream
database, runs plain PostgreSQL `SELECT` queries against that replica, and
returns either cacheable JSON snapshots or live Server-Sent Events.

With the `az-wire` feature, the same read-only operations are also available through a native
az-wire host. PgPaw can separately run as an embedded writable PostgreSQL primary for a Rust host;
embedded mode does not use logical replication.

Use PgPaw when you want:

- read-only SQL over HTTP without sending every read to upstream Postgres;
- shared snapshot URLs that browsers or CDNs can cache;
- realtime updates for the same SQL query shape;
- Postgres Row-Level Security to remain the source of truth for private data.

PgPaw does not handle writes. Keep writes in your application API, write to
Postgres, and let logical replication bring those changes back to PgPaw.

## Contents

- [Install](#install)
- [Quickstart](#quickstart)
- [How PgPaw Fits In](#how-pgpaw-fits-in)
- [Prepare Postgres](#prepare-postgres)
- [Run PgPaw](#run-pgpaw)
- [HTTP API](#http-api)
- [Realtime Streams](#realtime-streams)
- [Authorization And RLS](#authorization-and-rls)
- [SQL Rules](#sql-rules)
- [Caching Model](#caching-model)
- [Configuration](#configuration)
- [Logging](#logging)
- [Operations](#operations)
- [Troubleshooting](#troubleshooting)
- [Development](#development)

## Install

Install from Cargo:

```bash
cargo install pgpaw
```

When a supported prebuilt artifact is published, you can instead install the optional npm wrapper,
which downloads the native binary during installation:

```bash
npm install -g pgpaw
```

Check the binary:

```bash
pgpaw --help
```

## Quickstart

Prerequisites:

- Postgres 13 or newer.
- A Postgres user that can run the one-time setup in `pgpaw init`.
- PgPaw installed locally.

Prepare the upstream database:

```bash
pgpaw init \
  --pg-host 127.0.0.1 \
  --pg-port 5432 \
  --pg-user postgres \
  --pg-password "$POSTGRES_PASSWORD" \
  --pg-database myapp
```

If `pgpaw init` changes WAL settings, restart Postgres before starting PgPaw.
The command prints this clearly when a restart is required.

Start PgPaw:

```bash
pgpaw serve \
  --host 127.0.0.1 \
  --port 8080 \
  --data-dir ./pgpaw-data \
  --pg-host 127.0.0.1 \
  --pg-port 5432 \
  --pg-user postgres \
  --pg-password "$POSTGRES_PASSWORD" \
  --pg-database myapp
```

Check health:

```bash
curl http://127.0.0.1:8080/healthz
```

Run a snapshot query:

```bash
curl -i -X POST http://127.0.0.1:8080/query \
  -H "content-type: application/json" \
  -d '{"sql":"select id, email from users where id = 7"}'
```

Public queries return a `303 See Other` response:

```http
HTTP/1.1 303 See Other
Location: /q/{hash}/{version}
Cache-Control: no-store
```

Fetch the snapshot URL:

```bash
curl http://127.0.0.1:8080/q/{hash}/{version}
```

Open a live stream:

```bash
curl -N -X POST "http://127.0.0.1:8080/query?live=true" \
  -H "content-type: application/json" \
  -d '{"sql":"select id, status from orders order by id"}'
```

## How PgPaw Fits In

```text
Application writes
      |
      v
Upstream Postgres
  wal_level=logical
  publication=pgpaw_pub
  DDL event trigger, when permitted
      |
      | logical replication
      v
PgPaw local pglite replica
  SQL classifier
  version index
  query cache
  live diff hub
      |
      v
HTTP clients
  POST /query
  GET /q/{hash}/{version}
  POST /query?live=true
```

The important boundary is simple: PgPaw serves reads from a local replica. Your
application still owns writes, migrations, business logic, and upstream
Postgres.

## Prepare Postgres

`pgpaw init` is the guided setup command. It connects to upstream Postgres and
prepares logical replication.

It ensures:

1. `wal_level=logical`
2. `max_wal_senders >= 10`
3. `max_replication_slots >= 10`
4. publication `pgpaw_pub` exists for all tables, unless you pass another name
5. a DDL event trigger is installed, when the user has permission

The replication slot is not created by `init`. PgPaw creates the configured
slot automatically when `pgpaw serve` starts.

Run the setup with a custom publication name:

```bash
pgpaw init \
  --pg-host 127.0.0.1 \
  --pg-user postgres \
  --pg-password "$POSTGRES_PASSWORD" \
  --pg-database myapp \
  --publication app_read_pub
```

If you want a narrow publication, create it yourself first, then pass the same
name to `pgpaw init` and `pgpaw serve`:

```sql
create publication app_read_pub for table public.users, public.orders;
```

Tables that are not in the publication are not available through PgPaw.

## Run PgPaw

Most deployments use `pgpaw serve`:

```bash
pgpaw serve \
  --host 0.0.0.0 \
  --port 8080 \
  --data-dir /var/lib/pgpaw \
  --pg-host postgres.internal \
  --pg-port 5432 \
  --pg-user pgpaw \
  --pg-password "$PGPAW_POSTGRES_PASSWORD" \
  --pg-database app \
  --publication pgpaw_pub \
  --slot pgpaw_slot \
  --sslmode require
```

Use a persistent `--data-dir`. It stores the local pglite replica. If you
delete it, PgPaw must rebuild the replica from upstream.

For browser apps, set CORS explicitly:

```bash
pgpaw serve \
  --cors-origin https://app.example.com \
  --pg-database myapp
```

For local development, `--cors-origin "*"` is convenient. Avoid `*` for
production unless every response is intentionally public.

`serve` is also the default command, so this works:

```bash
pgpaw --pg-database myapp --port 8080
```

Use explicit subcommands in scripts because they make intent clearer:

```bash
pgpaw serve --pg-database myapp --port 8080
```

### Native az-wire

Build PgPaw with the `az-wire` feature to add an independent native az-wire listener:

```bash
cargo run --features az-wire -- serve \
  --host 127.0.0.1 \
  --port 8080 \
  --az-wire-host 127.0.0.1 \
  --az-wire-port 8788 \
  --az-wire-node pgpaw \
  --pg-database myapp
```

`--az-wire-port` is optional and has no default. Without it, serve remains HTTP-only. When set,
Actix HTTP and native az-wire bind independent listeners over shared PgPaw state; az-wire traffic
never passes through Actix. Startup reports readiness only after both listeners bind, and failure of
either listener rolls back startup.

The native subjects are `pgpaw.read`, `pgpaw.cursor`, and `pgpaw.live`. They preserve the HTTP
read-only SQL, authorization, cache, cursor, and live-query semantics. Mutating SQL is rejected.

### Embedded primary

Rust hosts can call `open_primary(&PrimaryConfig)` to start one writable embedded PostgreSQL primary,
then use `PrimaryHandle::dsn()` to create a direct PostgreSQL pool. With the `az-wire` feature,
`PrimaryHandle::attach_child(node, topology)` can attach read-only and realtime services as a
listenerless child of an existing az-wire node. The topology must contain a parent link and no host
listener. The direct pool remains the read/write database path; the child is additive and creates no
replica or second database.

## HTTP API

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/healthz` | Check replica status and watermark. |
| `POST` | `/query` | Run one read-only SQL query. |
| `GET` | `/q/{hash}/{version}` | Fetch a cached public snapshot. |
| `POST` | `/query?live=true` | Run a query and stream realtime changes. |

### GET /healthz

Healthy response:

```json
{ "status": "ok", "watermark": 12345678 }
```

Halted replica response:

```json
{ "status": "halted", "reason": "replication error details" }
```

Use this endpoint for readiness checks. A halted replica returns HTTP `503`.

### POST /query

Request body:

```json
{ "sql": "select id, email from users where id = 7" }
```

Public query response:

```http
HTTP/1.1 303 See Other
Location: /q/{hash}/{version}
Cache-Control: no-store
```

Private query response:

```http
HTTP/1.1 200 OK
Content-Type: application/json
Cache-Control: private, no-store
```

Private queries are returned inline because they depend on the caller's token
and must not enter the shared snapshot cache.

### GET /q/{hash}/{version}

Snapshot response:

```http
HTTP/1.1 200 OK
Content-Type: application/json
ETag: {hash}:{version}
Cache-Control: public, max-age=259200
```

The body is a JSON array:

```json
[
  { "id": 7, "email": "ada@example.com" }
]
```

If the cursor is unknown or has been evicted from PgPaw's in-process cache, the
server returns `404`. Query `/query` again to get the current cursor.

### Errors

Errors use a JSON envelope:

```json
{
  "name": "RejectedError",
  "message": "only read-only SELECT queries are cacheable; writes and DDL are not supported"
}
```

Common status codes:

| Status | Meaning |
| --- | --- |
| `400` | SQL parse error or rejected query. |
| `401` | Missing, malformed, expired, or unverifiable bearer token. |
| `403` | Postgres rejected the query under the caller's role. |
| `404` | Snapshot cursor is unknown. |
| `503` | Replica is halted. |

## Realtime Streams

Use `POST /query?live=true` to open an SSE stream.

Public query first event:

```text
data: {"type":"snapshot","url":"/q/{hash}/{version}","version":42}
```

Private query first event:

```text
data: {"type":"snapshot","rows":[{"id":1,"status":"open"}],"version":42}
```

Delta events:

```text
data: {"op":"insert","key":"7","row":{"id":7,"status":"open"},"txid":123}

data: {"op":"update","key":"7","row":{"id":7,"status":"paid"},"txid":124}

data: {"op":"delete","key":"7","txid":125}

data: {"op":"up-to-date","txid":125}
```

`up-to-date` marks the end of one transaction's diff cycle for that
subscription. The stream stays open.

If the live hub falls behind the internal replication broadcast, PgPaw emits:

```text
data: {"op":"reset"}
```

After a reset, reconnect and request a fresh snapshot.

For the most stable live keys, use single-table queries that include a primary
key. Multi-table queries and queries without a primary key use a content hash as
the row key.

## Authorization And RLS

PgPaw decides whether a query is public or private from the replicated schema:

- A table is public only when Row-Level Security is disabled and `PUBLIC` has
  `SELECT` on that table.
- A table is private when RLS is enabled or `PUBLIC` lacks `SELECT`.
- A query is private if any table it touches is private.

Public queries need no token and use the shared snapshot cache. Private queries
require a bearer token.

Configure exactly one JWT verification method:

```bash
pgpaw serve \
  --jwt-secret "$JWT_SECRET" \
  --jwt-role-claim role \
  --pg-database myapp
```

Or use a PEM public key for RS256 or ES256 tokens:

```bash
pgpaw serve \
  --jwt-public-key "$JWT_PUBLIC_KEY" \
  --jwt-role-claim role \
  --pg-database myapp
```

`--jwt-jwks-url` is present in the CLI but JWKS verification is not implemented
yet. Use `--jwt-secret` or `--jwt-public-key`.

The role claim defaults to `role`. PgPaw runs private queries under that
Postgres role. The full claims JSON is also available to RLS policies through
`request.jwt.claims`.

Example RLS policy:

```sql
create role member;

alter table documents enable row level security;

create policy documents_by_org on documents
  for select
  to member
  using (
    org_id = (current_setting('request.jwt.claims', true)::json->>'org_id')::int
  );
```

Token example:

```json
{
  "role": "member",
  "org_id": 7,
  "exp": 1893456000
}
```

Private request example:

```bash
curl -i -X POST http://127.0.0.1:8080/query \
  -H "content-type: application/json" \
  -H "authorization: Bearer $TOKEN" \
  -d '{"sql":"select id, title from documents order by id"}'
```

## SQL Rules

PgPaw accepts one read-only PostgreSQL `SELECT` statement over replicated
tables.

Accepted examples:

```sql
select id, email from users where id = 7;

select u.id, u.email, count(o.id) as order_count
from users u
left join orders o on o.user_id = u.id
where u.id = 7
group by u.id, u.email;
```

Rejected input includes:

- multiple statements, such as `select 1; select 2`;
- writes: `insert`, `update`, `delete`, `merge`;
- DDL: `create`, `alter`, `drop`;
- locking reads: `for update`, `for share`;
- `select into`;
- references to tables outside the publication;
- side-effecting functions such as `nextval`, `setval`, and advisory lock
  functions;
- volatile functions such as `now`, `random`, `clock_timestamp`,
  `statement_timestamp`, `gen_random_uuid`, and `uuid_generate_v4`.

For cache precision, PgPaw recognizes equality filters such as:

```sql
where id = 7
```

On a single table, equality on a primary key anchors the query version to that
row. If the table uses `REPLICA IDENTITY FULL`, equality on any replicated
column can be used as an anchor. Other queries fall back to table-level
invalidation.

## Caching Model

PgPaw computes a fingerprint from the parsed SQL and combines it with a
replication-derived version:

```text
cache key = sql_fingerprint + ":" + version
```

The version changes when logical replication reports a relevant upstream
commit. This gives PgPaw two useful properties:

- The same SQL returns the same snapshot cursor while relevant data is
  unchanged.
- A relevant upstream write produces a new cursor.

Public snapshots are stored in PgPaw's in-process query cache and served through
`GET /q/{hash}/{version}` with:

```http
Cache-Control: public, max-age=259200
```

The server-side cache is bounded by `--cache-size-bytes`. If a snapshot is
evicted, the old cursor returns `404`; clients should call `/query` again.

## Configuration

All common flags have matching environment variables.

### `pgpaw init`

| Flag | Env | Default | Description |
| --- | --- | --- | --- |
| `--pg-host` | `UPSTREAM_HOST` | `127.0.0.1` | Upstream Postgres host. |
| `--pg-port` | `UPSTREAM_PORT` | `5432` | Upstream Postgres port. |
| `--pg-user` | `UPSTREAM_USER` | `postgres` | Upstream Postgres user. |
| `--pg-password` | `UPSTREAM_PASSWORD` | empty | Upstream Postgres password. |
| `--pg-database` | `UPSTREAM_DATABASE` | `postgres` | Upstream database. |
| `--publication` | `UPSTREAM_PUBLICATION` | `pgpaw_pub` | Publication to create or verify. |

### `pgpaw serve`

| Flag | Env | Default | Description |
| --- | --- | --- | --- |
| `--host` | `PGPAW_HOST` | `127.0.0.1` | PgPaw HTTP bind host. |
| `--port` | `PGPAW_PORT` | `8080` | PgPaw HTTP bind port. |
| `--data-dir` | `PGPAW_DATA_DIR` | `./cache-data` | Local pglite replica directory. |
| `--max-connections` | `PGPAW_MAX_CONNECTIONS` | `8` | Local replica connection pool size. |
| `--cache-size-bytes` | `PGPAW_CACHE_SIZE_BYTES` | `268435456` | Query cache byte budget. |
| `--pg-host` | `UPSTREAM_HOST` | `127.0.0.1` | Upstream Postgres host. |
| `--pg-port` | `UPSTREAM_PORT` | `5432` | Upstream Postgres port. |
| `--pg-user` | `UPSTREAM_USER` | `postgres` | Upstream Postgres user. |
| `--pg-password` | `UPSTREAM_PASSWORD` | empty | Upstream Postgres password. |
| `--pg-database` | `UPSTREAM_DATABASE` | `postgres` | Upstream database. |
| `--publication` | `UPSTREAM_PUBLICATION` | `pgpaw_pub` | Publication to replicate. |
| `--slot` | `UPSTREAM_SLOT` | `pgpaw_slot` | Logical replication slot name. |
| `--sslmode` | `UPSTREAM_SSLMODE` | `disable` | `disable`, `prefer`, `require`, or `verify-full`. |
| `--jwt-secret` | `JWT_SECRET` | unset | HS256 verification secret. |
| `--jwt-public-key` | `JWT_PUBLIC_KEY` | unset | RS256 or ES256 PEM public key. |
| `--jwt-jwks-url` | `JWT_JWKS_URL` | unset | Reserved; not implemented yet. |
| `--jwt-role-claim` | `JWT_ROLE_CLAIM` | `role` | Claim containing the Postgres role. |
| `--cors-origin` | `CORS_ORIGIN` | unset | Browser origin, comma-separated origins, or `*`. |

### `pgpaw primary`

`primary` runs an embedded writable Postgres over TCP. It is not required for
normal upstream-replica mode.

```bash
pgpaw primary \
  --data-dir ./primary-data \
  --primary-listen 127.0.0.1 \
  --primary-port 5432
```

| Flag | Env | Default | Description |
| --- | --- | --- | --- |
| `--data-dir` | `PGPAW_DATA_DIR` | `./cache-data` | Embedded Postgres data directory. |
| `--max-connections` | `PGPAW_MAX_CONNECTIONS` | `8` | Connection pool size. |
| `--primary-listen` | `PRIMARY_LISTEN` | `127.0.0.1` | TCP listen address. |
| `--primary-port` | `PRIMARY_PORT` | `5432` | TCP port. |

## Logging

PgPaw writes operational logs to stderr in logfmt-style lines. It logs at
`INFO`, `WARN`, and `ERROR`; it does not require `DEBUG` logs to understand
whether the service is working.

Startup logs show the bind address, HTTP URL, upstream Postgres target,
publication, replication slot, data directory, cache size, auth state, schema
scan counts, CDC startup, and readiness:

```text
ts=2026-06-29T10:00:07.656Z level=INFO pid=65538 target=pgpaw event=command_start command=serve
ts=2026-06-29T10:00:07.657Z level=INFO pid=65538 target=pgpaw event=server_starting bind_addr=127.0.0.1:8080 data_dir="./pgpaw-data" upstream_host=127.0.0.1 upstream_port=5432 upstream_database=myapp publication=pgpaw_pub slot=pgpaw_slot sslmode=disable auth_configured=false cors_origin=None
ts=2026-06-29T10:00:08.120Z level=INFO pid=65538 target=pgpaw::http::server event=http_server_listening bind_addr=127.0.0.1:8080 url=http://127.0.0.1:8080
ts=2026-06-29T10:00:08.121Z level=INFO pid=65538 target=pgpaw event=server_ready bind_addr=127.0.0.1:8080 health_path=/healthz query_path=/query
```

Request and query logs show access events, query classification, public/private
decision, snapshot cursor/version, cache hits, live subscriptions, CDC
transactions, and health checks:

```text
ts=2026-06-29T10:00:10.001Z level=INFO pid=65538 target=pgpaw::http::query event=query_classified fingerprint=9a4f tables=users live=false scope=public
ts=2026-06-29T10:00:10.002Z level=INFO pid=65538 target=pgpaw::cache event=query_cache_get_or_compute result=hit key=9a4f:42 bytes=128
ts=2026-06-29T10:00:10.003Z level=INFO pid=65538 target=pgpaw::http::query event=query_snapshot scope=public fingerprint=9a4f tables=users version=42 cursor=/q/9a4f/42 response=redirect snapshot_bytes=128
ts=2026-06-29T10:00:10.004Z level=INFO pid=65538 target=actix_web::middleware::logger event=http_request remote_addr=127.0.0.1 request="POST /query HTTP/1.1" status=303 response_bytes=0 duration_ms=2 user_agent="curl/8.7.1"
```

Warnings and errors are logged for rejected SQL, missing tokens, cursor misses,
replica halt, DDL-trigger setup failures, bind failures, and other operator
action items.

PgPaw intentionally does not log raw bearer tokens, Postgres passwords, or raw
SQL text. Query logs use the SQL fingerprint and table list so production logs
stay useful without exposing sensitive query literals.

Capture logs with your process supervisor:

```bash
pgpaw serve ... 2>&1 | tee pgpaw.log
```

With systemd:

```bash
journalctl -u pgpaw -f
```

With Docker:

```bash
docker logs -f <container>
```

## Operations

Production checklist:

- Keep `--data-dir` on persistent storage.
- Run `pgpaw init` before the first `serve`.
- Restart Postgres when `init` says WAL settings changed.
- Use a dedicated upstream Postgres user for PgPaw in production.
- Set `--cors-origin` to exact browser origins.
- Configure JWT verification before serving private RLS-protected data.
- Monitor `/healthz`; treat `503` as not ready.
- Size `--cache-size-bytes` for your hot public snapshots.
- If the DDL event trigger was not installed, restart PgPaw after schema changes
  that affect queried tables.

Schema changes:

- With `CREATE PUBLICATION ... FOR ALL TABLES`, new tables are automatically
  part of the publication.
- With a narrow publication, add new tables manually:

```sql
alter publication pgpaw_pub add table public.new_table;
```

Security changes:

- Enabling RLS or revoking `PUBLIC SELECT` makes affected queries private.
- Granting `PUBLIC SELECT` and disabling RLS makes affected queries public.
- PgPaw checks table privacy from the replicated schema, so allow replication to
  catch up before expecting new behavior.

## Troubleshooting

### `wal_level` must be `logical`

Run:

```bash
pgpaw init --pg-database myapp
```

Then restart Postgres if `init` changed WAL settings.

### Publication does not exist

Run `pgpaw init` with the same `--publication` that `pgpaw serve` uses, or pass
an existing publication name to both commands.

### Table is not available in this cache

The table is not present in the replicated publication, or PgPaw has not caught
up yet. Add the table to the publication and wait for replication.

### Private query returns `401`

The query touches a table with RLS enabled or without `PUBLIC SELECT`. Send a
valid bearer token and configure PgPaw with `--jwt-secret` or
`--jwt-public-key`.

### Token is presented but JWT verification is not configured

Start PgPaw with one JWT verification source. If all data is public, do not send
an `Authorization` header.

### `--jwt-jwks-url` fails at startup

JWKS verification is not implemented yet. Use `--jwt-secret` or
`--jwt-public-key`.

### Old snapshot URL returns `404`

The snapshot may have been evicted from PgPaw's in-process cache. Repeat the
`POST /query` request to get the current cursor.

### Live stream emits `reset`

Reconnect and request a fresh snapshot. `reset` means PgPaw intentionally
dropped the subscription because it could not safely continue diffing from the
previous state.

## Development

Build and check the Rust workspace:

```bash
cargo check --workspace
cargo test --workspace
```

Build the CLI:

```bash
cargo build --bin pgpaw
```

Run the Node integration tests from `integration-tests/node` after building the
release binary they expect:

```bash
cargo build --release --bin pgpaw
cd integration-tests/node
pnpm install
pnpm test
```

Example apps live in:

- `examples/nextjs-todos`
- `examples/nextjs-project-board`

## License

MIT. See [LICENSE](LICENSE).
