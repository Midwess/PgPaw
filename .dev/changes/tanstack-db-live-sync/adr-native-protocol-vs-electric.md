# ADR: Native PgPaw wire protocol instead of Electric-protocol compatibility

**Status**: accepted
**Date**: 2026-06-16

## Context

To make PgPaw "TanStack-compatible like Electric" there are two ways to let a
TanStack DB collection sync from it:

1. **Speak Electric's wire protocol** — expose `/v1/shape` with
   `offset`/`handle`/`operation`/`control`/`409`, so the official
   `@tanstack/electric-db-collection` plugs in unchanged. Electric shapes are
   single-table (`table` + `where` + `columns`).
2. **Ship a native PgPaw collection** — keep PgPaw's existing plain-SQL `POST
   /query` + SSE wire and provide our own `@pgpaw/tanstack-db` collection adapter.

## Decision

Ship the **native PgPaw collection** (option 2).

## Rationale

- PgPaw's entire differentiator is **arbitrary read-only SQL, including joins**.
  Electric's shape model is single-table; adopting its protocol would force every
  synced collection back into single-table shapes and discard PgPaw's reason to
  exist.
- The architecture is still Electric-shaped (read-only engine, write-to-your-API,
  txid reconciliation, framework-agnostic backend, thin client adapter), so the
  mental model and the client ergonomics are familiar to Electric users.
- The backend stays protocol-neutral: it gains `txid`, authenticated live, and a
  `reset` event — all useful to any client, not just TanStack.

## Consequences

- **The official `@tanstack/electric-db-collection` does NOT work against PgPaw.**
  Users adopt `@pgpaw/tanstack-db` instead.
- No durable per-shape log, so no offset-precise resume; a lagged client
  re-snapshots via the `reset` event rather than gap-filling. Acceptable given the
  arbitrary-SQL choice (you cannot keep a compact append-only log per arbitrary
  join the way you can per fixed shape).
- This is hard to reverse once the package is published: it defines the public
  client API and the on-the-wire contract third parties depend on.

## Alternatives considered

- **Both** (native + an Electric-protocol shim): larger scope; deferred. The shim
  could be added later without changing the native package.
