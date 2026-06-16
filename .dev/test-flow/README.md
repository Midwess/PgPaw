# Test Flow

Folder-based, notebook-style test cases for PgPaw. Each subfolder holds one `test-case.md` (the notebook) plus dated `test-<date>.md` observation logs.

Run a case with `/dev-workflow:do-test-flow .dev/test-flow/<test-id>/`.

## Test Inventory

| Test ID | Title | Tier | Component | Created |
|---------|-------|------|-----------|---------|
| jwt-rls-enforcement | JWT-scoped /query enforces upstream RLS so each tenant sees only its own rows, with correct 303/200/401/403 handling | live | auth | 2026-06-16 |
| replica-ddl-propagation | Security DDL applied upstream after launch propagates into the replica and re-classifies public→private with no restart | live | replica | 2026-06-16 |
| classifier-fail-closed | Public/private classifier fails closed on every ambiguous edge — never serves access-controlled data as a public snapshot | live | classify | 2026-06-16 |
| jwt-verification-modes | JWT verifier accepts only well-formed HS256, rejects expired/bad-sig/alg-none/missing-role/malformed with 401; no-jwt server fails closed | live | auth | 2026-06-16 |
| realtime-data | Live /query?live=true pushes a follow-up SSE event when an upstream INSERT replicates in; version bumps | live | live | 2026-06-16 |
| query-redirect | Public /query is a content-addressed redirect cache — 303→/q/{hash}/{version}, cacheable ETag 200, idempotent hash, version bump, 404 on unknown cursor | live | cache | 2026-06-16 |
