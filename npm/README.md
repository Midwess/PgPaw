# pgpaw

[![Crates.io](https://img.shields.io/crates/v/pgpaw)](https://crates.io/crates/pgpaw)
[![npm](https://img.shields.io/npm/v/pgpaw)](https://www.npmjs.com/package/pgpaw)

Read-only Postgres cache + realtime server over an embedded
[pglite](https://crates.io/crates/pglite-rs) logical replica.

This npm package ships a thin launcher; on install it downloads the prebuilt
`pgpaw` binary for your platform from the matching
[GitHub Release](https://github.com/Midwess/PgPaw/releases).

## Install

PgPaw is a single static binary. Install it with whichever toolchain you have:

```bash
# via npm (downloads the prebuilt binary for your platform)
npm install -g pgpaw

# via Cargo (builds from source)
cargo install pgpaw
```

Prebuilt binaries: Linux x86_64 and macOS Apple Silicon. Other platforms build
from source via `cargo install pgpaw`.

## Quickstart

Prereqs: `pgpaw` installed and a reachable Postgres 13+ with superuser access
(for the one-time `init` step).

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

Full documentation: https://github.com/Midwess/PgPaw
