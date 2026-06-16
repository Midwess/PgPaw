# Delta for TanStack DB Client (`@pgpaw/tanstack-db`)

## ADDED Requirements

### Requirement: Native collection options creator

The package SHALL export `pgpawCollectionOptions(config)` returning a
`collectionOptions` object consumable by TanStack DB's `createCollection`, where
`config` accepts `url`, `sql`, `getKey`, optional `headers`, and optional
`onInsert`/`onUpdate`/`onDelete` write handlers.

#### Scenario: collection options shape

- WHEN `pgpawCollectionOptions({ url, sql, getKey })` is called
- THEN it returns an object exposing `getKey`, a `sync` config, and a `utils`
  object containing `awaitTxId`

### Requirement: Initial load from the live stream

The sync engine SHALL open `POST {url}/query?live=true` with the configured `sql`
and headers, load the initial rows from the first event, then mark the collection
ready.

#### Scenario: public first event (pointer)

- WHEN the first event is `{"type":"snapshot","url":U,...}`
- THEN the engine fetches `GET {url}{U}`, writes each row as an `insert` inside a
  single `begin`/`commit`, then calls `markReady`

#### Scenario: private first event (inline)

- WHEN the first event is `{"type":"snapshot","rows":R,...}`
- THEN the engine writes each row of `R` as an `insert` inside a single
  `begin`/`commit`, then calls `markReady` (no extra fetch)

### Requirement: Apply live deltas

The sync engine SHALL apply each subsequent delta to the collection.

#### Scenario: delta applied

- WHEN an `insert` / `update` / `delete` event arrives
- THEN the engine performs the matching `write` inside a `begin`/`commit`

#### Scenario: txid recorded

- WHEN any event carries a `txid`
- THEN the engine records it so `awaitTxId` can resolve

### Requirement: Reset re-synchronizes

The sync engine SHALL re-synchronize on a `reset` event.

#### Scenario: reset clears and reloads

- WHEN a `{"op":"reset"}` event arrives
- THEN the engine truncates the collection and re-runs the initial load

### Requirement: Optimistic write confirmation by txid

Write handlers SHALL be awaited until the transaction id they return appears in
the synced stream, matched on the low 32 bits.

#### Scenario: write resolves when txid is seen

- WHEN `onInsert` returns `{ txid: X }` (e.g. from `pg_current_xact_id()`)
- THEN the handler does not resolve until a synced event carries a txid equal to
  `X & 0xffffffff`

#### Scenario: awaitTxId times out

- WHEN the expected txid never arrives within the timeout
- THEN `awaitTxId` rejects rather than hanging forever
