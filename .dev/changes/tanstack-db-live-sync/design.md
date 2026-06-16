# Design: tanstack-db-live-sync

## Overview

PgPaw becomes a transaction-aware live source and the new `@pgpaw/tanstack-db`
package adapts it to TanStack DB. The model mirrors ElectricSQL (read-only sync
engine, writes to your own API, optimistic state reconciled by transaction id)
but over PgPaw's plain-SQL SSE wire rather than Electric's shape protocol.

## Wire protocol (the SSE the library consumes)

`POST /query?live=true`, `Authorization: Bearer <jwt>` when access-controlled.

First event — public (unchanged, CDN-friendly):

```
data: {"type":"snapshot","url":"/q/9a4f/42","version":42}
```

First event — private/RLS (new; no cacheable pointer exists):

```
data: {"type":"snapshot","rows":[{"id":7,"status":"paid"}],"version":42}
```

Deltas — `txid` added (the CDC `u32`):

```
data: {"op":"insert","key":"7","row":{...},"txid":7654321}
data: {"op":"update","key":"7","row":{...},"txid":7654322}
data: {"op":"delete","key":"7","txid":7654323}
data: {"op":"up-to-date","txid":7654323}
data: {"op":"reset"}
```

`up-to-date` carries the txid for every commit that touches the subscription's
tables, even when the diff is empty. `reset` (on CDC lag) tells the client to
truncate and re-snapshot.

## Key Decisions

### Decision 1: native PgPaw wire vs Electric-protocol compatibility

See `adr-native-protocol-vs-electric.md`. Chosen: native wire + own collection,
to keep PgPaw's arbitrary-SQL (joins) instead of Electric's single-table shapes.

### Decision 2: write confirmation by txid vs awaitMatch

**Context:** TanStack DB drops the optimistic overlay once a write "comes back"
through sync. The match can be by transaction id or by row content.
**Options:**
1. `awaitTxId` — match the upstream transaction id. Robust; needs PgPaw to emit it.
2. `awaitMatch` — match by row content. Zero backend change; brittle on equal rows.
**Decision:** `awaitTxId`. PgPaw already holds `CommittedTransaction.xid`; emitting
it is small and gives a real transaction handshake. Client matches the low 32 bits
of `pg_current_xact_id()` against the CDC `xid`.

### Decision 3: resume by re-snapshot vs durable per-shape log

**Context:** A lagged client must not silently diverge.
**Options:**
1. Durable per-shape log + offset-precise resume (Electric's model). Not feasible
   for arbitrary SQL/joins.
2. `reset` → client truncates and reloads the snapshot.
**Decision:** `reset`. Cheap, correct, and consistent with the arbitrary-SQL choice.

### Decision 4: private initial load inline vs pointer

**Context:** Private queries are never cached and have no `/q` pointer.
**Decision:** the private live first event carries inline `rows`. The library
branches on `url?` vs `rows?`. Public keeps the CDN-cacheable pointer.

## Data Model

No schema changes. The only new on-the-wire field is `txid` (`u32`) plus the
`reset` and inline-`rows` event shapes.

## API Changes

- `POST /query?live=true` over an access-controlled query: now `200`
  `text/event-stream` (was `403`). Token required and enforced under the role.
- Live delta and `up-to-date` events gain `txid`.
- New `reset` event.

## Security Considerations

- RLS live executes every recompute via `query_json_as(role, claims)`, so the
  embedded replica's own RLS policies filter rows per the token — same authority
  as the existing private read path. PgPaw only verifies the token.
- The token is supplied on the live request; an expired/invalid token is rejected
  before the stream opens (existing `AuthOutcome`).
- No new caching of private data: private live rows are streamed inline, never
  written to the shared `/q` cache.
