---
test_id: example-nextjs-todos
run_date: 2026-06-16
run_time_utc: "17:40:00Z"
runner: claude-opus (main thread) + real Chromium via Playwright
sandbox_or_env: "docker postgres:16 :5433, pgpaw serve :8080 --cors-origin '*', next dev :3000, chromium-headless"
duration_secs: 240
result: pass
exit_reason: ""
related_commit: "4eb4947"
---

## Summary

Re-ran the example in a **real browser** (Chromium via Playwright) after the
partial run on 2026-06-16. Drove the actual UI — page render, add, toggle, a live
direct-DB update, and delete — each cross-checked against Postgres. All passed,
twice (determinism re-run). Getting here required one product fix the browser
surfaced: **PgPaw had no CORS**, so the browser could not call it cross-origin; a
Next rewrite proxy was tried but it gzip-buffers SSE (browsers send
`Accept-Encoding: gzip`), so live deltas never flushed. Added a `--cors-origin`
flag to PgPaw; the browser now calls PgPaw directly and the live stream works.

## Steps Executed

### Step 1: Stack up (PG :5433, pgpaw init/serve, lib build+link, next dev)
**Primary observation**: `/healthz` → `{"status":"ok","watermark":26839464}`. Next dev `Ready`. Page `GET / 200`.
**Awareness check**: navigated via `localhost:3000` (not `127.0.0.1`) — Next dev blocks `/_next/` chunks for a non-allowed dev origin (would blank the page). pgpaw run with `--cors-origin '*'`.
**Judgment**: pass

### Step 2: Page render (real browser)
**Action run**: `chromium → goto http://localhost:3000 (domcontentloaded)`; wait for `heading "Todos"`.
**Primary observation**: heading rendered → the client-only `dynamic({ ssr: false })` component hydrated and mounted. No `pageerror`.
**Awareness check**: only console noise was Next HMR WebSocket — harmless.
**Judgment**: pass

### Step 3: Add (optimistic + persisted)
**Action run**: type "walk the dog" → click Add.
**Primary observation**: row appears in the list immediately; psql → `walk the dog|f`.
**Judgment**: pass

### Step 4: Toggle (optimistic + txid-confirmed)
**Action run**: click the row checkbox.
**Primary observation**: psql → `walk the dog|t`. Optimistic flip held (no rollback), i.e. the PATCH txid was observed in the live stream.
**Judgment**: pass

### Step 5: LIVE — direct DB update reflects in UI (the previously-failing step)
**Action run**: `update todos set title='renamed live' where id=...` (direct psql), no page reload.
**Primary observation**: the rendered row text changed to "renamed live" within seconds — the SSE delta reached the browser collection and re-rendered.
**Awareness check**: this is the proof CORS-direct works; the proxy path failed here (gzip buffering).
**Judgment**: pass

### Step 6: Delete (via UI)
**Action run**: click the row's ✕.
**Primary observation**: row removed from UI; psql `count` → 0.
**Judgment**: pass

### Step 7: Determinism re-run
Ran the whole browser flow a second time on a cleared DB → identical PASS.

## Environment Observations

- PgPaw `--cors-origin '*'` → OPTIONS preflight returns `200` with
  `access-control-allow-origin` reflected; POST/SSE carries the header.
- Restarting `pgpaw serve` on a reused data-dir failed (`postmaster failed to
  start`) — the killed serve's embedded postgres held the dir lock; a fresh
  `--data-dir` fixed it. Worth a notebook awareness item.
- The browser must use `localhost` (Next dev `allowedDevOrigins`); `127.0.0.1`
  blanks the page by blocking dev chunks.

## Evidence

- E2E driver: `examples/nextjs-todos/e2e.mjs` (Playwright + psql cross-checks), run twice → `RESULT: PASS`, exit 0.
- CORS preflight: `access-control-allow-origin: http://localhost:3000` on `OPTIONS /query?live=true`.
- DB transitions per step: `walk the dog|f` → `|t` → `renamed live` → count 0.

## Deviations

- PgPaw port remapped 5432→5433 (5432 host-occupied).
- Example linked via `link:` dep + a temporary `playwright` devDep + `e2e.mjs`
  driver — all test-only, reverted/removed in cleanup.
- Browser uses bundled Chromium (the Playwright MCP wanted system Chrome, which
  needs sudo to install) — driven by a local Playwright script, not the MCP.

## Root Cause Analysis (resolved)

The live cross-origin path failed because PgPaw exposed no CORS, and the
same-origin Next rewrite proxy buffers SSE under gzip. Fixed at the product
level: `--cors-origin` (actix-cors) lets browsers call PgPaw directly, matching
how Electric ships its shape API. With CORS, direct PgPaw SSE is uncompressed
and streams, so live deltas reach the browser. The SSR crash (bug #1) and stale
binary (bug #2) from the prior run were already fixed.

## Next Actions

- [x] Add PgPaw `--cors-origin` (browser reachability).
- [x] Full browser pass (render + add + toggle + live + delete), x2.
- [ ] Run the project-board browser test the same way (`--cors-origin`, localhost).
- [ ] Document `--cors-origin` in PgPaw + example READMEs (done in this change).
