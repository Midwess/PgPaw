# PgPaw

This npm package installs the `pgpaw` native CLI.

PgPaw is a read-only HTTP and realtime layer for Postgres. It keeps a local
pglite logical replica of your upstream database, runs plain PostgreSQL
`SELECT` queries against that replica, and returns either cacheable JSON
snapshots or live Server-Sent Events.

For the full guide, see the repository README:

https://github.com/Midwess/PgPaw#readme

## Install

```bash
npm install -g pgpaw
```

The postinstall script downloads a prebuilt binary when one is available.
Current prebuilt targets:

- Linux x86_64
- macOS Apple Silicon

If your platform is not supported by the npm prebuilt binary, install from
source instead:

```bash
cargo install pgpaw
```

## Check The CLI

```bash
pgpaw --help
```

## Quick Start

Prepare upstream Postgres:

```bash
pgpaw init \
  --pg-host 127.0.0.1 \
  --pg-port 5432 \
  --pg-user postgres \
  --pg-password "$POSTGRES_PASSWORD" \
  --pg-database myapp
```

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

Query PgPaw:

```bash
curl -i -X POST http://127.0.0.1:8080/query \
  -H "content-type: application/json" \
  -d '{"sql":"select id, email from users where id = 7"}'
```

Open a live stream:

```bash
curl -N -X POST "http://127.0.0.1:8080/query?live=true" \
  -H "content-type: application/json" \
  -d '{"sql":"select id, status from orders order by id"}'
```

## Documentation

The full guide covers:

- Postgres setup with `pgpaw init`
- public and private query behavior
- Row-Level Security and JWT verification
- HTTP endpoints
- live SSE events
- cache invalidation
- production configuration
- troubleshooting

Read it here:

https://github.com/Midwess/PgPaw#readme

## License

MIT.
