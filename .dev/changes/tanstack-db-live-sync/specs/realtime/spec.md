# Delta for Realtime

## ADDED Requirements

### Requirement: Transaction id on live deltas

The live SSE stream SHALL include the upstream Postgres transaction id (`txid`,
the low 32 bits of the committing transaction) on every row delta and on the
terminal `up-to-date` event, so a client can reconcile an optimistic write with
its confirming sync event.

#### Scenario: delta carries txid

- WHEN a committed upstream transaction with xid `T` changes a row in a live
  subscription's result set
- THEN the emitted `insert` / `update` / `delete` event includes `"txid": T`

#### Scenario: up-to-date carries txid even with an empty diff

- WHEN a committed transaction with xid `T` touches a subscription's table but
  produces no visible change to its result set (no-op update, or a row outside
  the WHERE filter)
- THEN an `up-to-date` event with `"txid": T` is still emitted for that
  subscription

### Requirement: Authenticated live streaming of access-controlled queries

The system SHALL allow live streaming of an access-controlled (RLS) query for an
authenticated principal, recomputing each delta under the token's role.

#### Scenario: RLS query streams live under the role

- WHEN a request to `POST /query?live=true` carries a valid bearer token and the
  query is access-controlled
- THEN the stream opens with `200 text/event-stream` (not `403`)
- AND each recompute runs as the token's role via the RLS-enforcing execution
  path, so only rows the role may see are diffed and streamed

#### Scenario: missing token on an access-controlled live query

- WHEN a request to `POST /query?live=true` for an access-controlled query has no
  valid token
- THEN it is rejected with `401` and no stream opens

### Requirement: Inline initial rows for private live

The first event of a live stream for an access-controlled query SHALL carry the
initial result rows inline, because private queries have no cacheable snapshot
pointer.

#### Scenario: private first event is inline rows

- WHEN an access-controlled live stream opens
- THEN the first event is `{"type":"snapshot","rows":[...],"version":<v>}` with
  the initial rows computed under the role

#### Scenario: public first event is unchanged

- WHEN a public live stream opens
- THEN the first event is `{"type":"snapshot","url":"/q/{hash}/{version}","version":<v>}`
  (the CDN-cacheable pointer, unchanged)

### Requirement: Reset event on CDC lag

The system SHALL emit a `reset` control event to live subscribers when the change
feed lags and transactions are dropped, so clients re-synchronize instead of
serving stale data indefinitely.

#### Scenario: lag triggers reset

- WHEN the change feed reports a lagged/dropped batch
- THEN each live subscription receives `{"op":"reset"}`
- AND the client is expected to discard its state and re-load the snapshot
