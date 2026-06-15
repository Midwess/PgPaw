# Test Flow

Folder-based, notebook-style test cases for PgPaw. Each subfolder holds one `test-case.md` (the notebook) plus dated `test-<date>.md` observation logs.

Run a case with `/dev-workflow:do-test-flow .dev/test-flow/<test-id>/`.

## Test Inventory

| Test ID | Title | Tier | Component | Created |
|---------|-------|------|-----------|---------|
| jwt-rls-enforcement | JWT-scoped /query enforces upstream RLS so each tenant sees only its own rows, with correct 303/200/401/403 handling | live | auth | 2026-06-16 |
