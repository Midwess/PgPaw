# PgPaw — Node integration tests

End-to-end tests of [`@pgpaw/tanstack-db`](../../packages/tanstack-db) against a
**real PgPaw stack**. A `vitest` `globalSetup` boots the whole thing and tears it
down:

```
docker postgres:16 (wal_level=logical) → pgpaw init → pgpaw serve --jwt-secret → fixtures
```

Tests drive the actual collection headless (`@tanstack/db` + the adapter, no
browser): `collection.preload()`, optimistic `insert/update/delete`, and direct
upstream writes via `pg` to exercise live deltas.

## Prerequisites

- Docker running.
- A built binary at `../../target/release/pgpaw` (`cargo build --release --bin pgpaw`).

## Run

```bash
pnpm install --ignore-workspace
pnpm test
```

`--ignore-workspace` keeps the parent monorepo's JS workspace out; the package
consumes the local library via `link:../../packages/tanstack-db`.

## Coverage (the hardest cases)

| Suite | What it proves |
|-------|----------------|
| `public-crud` | snapshot preload, then live insert / update / delete |
| `optimistic-txid` | optimistic mutations confirmed by `awaitTxId`; timeout rejects |
| `rls-multitenant` | per-token RLS isolation (org A vs B); a tenant's live insert never reaches the other; **private live under the role**; 401 without / with expired token |
| `joined-collection` | one collection over a 3-table join; a parent rename propagates live to joined rows; row updates + live inserts |
| `classifier` | read-only `SELECT` accepted (303); writes / DDL / multi-statement / unknown-table rejected (400) |

Ports: Postgres `5434`, PgPaw `8085` (chosen to avoid the examples' `5433`/`8080`).
