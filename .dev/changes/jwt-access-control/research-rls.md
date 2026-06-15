# RLS Best Practices — Research (cited)

Source: deep-research run 2026-06-15 (19 sources, 84 claims, 25 verified, 24 confirmed / 1 refuted). Primary sources: postgresql.org docs, Supabase RLS + RLS-performance docs.

## Bottom line
RLS-as-primary-authorization in the PostgREST/Supabase model (`SET LOCAL ROLE <non-superuser JWT role>` + `SET LOCAL request.jwt.claims`, policies reading `current_setting('request.jwt.claims', true)`) is sound **iff** four things hold: default-deny policy design, the `(select …)` perf wrap, **non-superuser execution** (the dominant footgun), and out-of-band creation of policies/roles/helpers on the replica.

## Verified findings

### Policy design
- **Combination:** permissive policies OR together; restrictive AND. Effective predicate = `(perm1 OR perm2 …) AND (restr1 AND restr2 …)`. **At least one permissive policy must exist** — restrictive-only = zero rows. (PG docs `sql-createpolicy`, `ddl-rowsecurity`; PG10→18.)
- **Default-deny:** RLS off by default; `ENABLE ROW LEVEL SECURITY` then "no applicable policy ⇒ default-deny, no rows." Safe baseline. (PG docs; Supabase.)
- **USING vs WITH CHECK:** USING filters visible rows (false/null hides, no error); WITH CHECK validates new rows (false/null errors). SELECT/DELETE can't have WITH CHECK; INSERT can't have USING. For a read API only SELECT+USING matters. Scope policies per-role via `TO` (defaults PUBLIC). (PG docs.)
- **Multi-tenant shape:** ENABLE+FORCE RLS, a per-role **permissive** SELECT policy, plus a **RESTRICTIVE** tenant predicate (`tenant_id = <claim>`) that every permissive grant must AND-satisfy. Composes with column GRANT/REVOKE as a second layer. (PG docs; Crunchy/Permit/MakerKit.)

### Performance
- **#1 — initPlan wrap:** reference `auth.uid()`/`current_setting()` as a scalar `(select …)` so the optimizer caches it once per statement instead of per row. **STABLE alone is insufficient** — a USING expr defaults to a per-row SubPlan; the subquery wrap makes it an InitPlan. Supabase benchmarks: ~95% (`auth.uid()`), up to 99.94–99.99% (table-join / SECURITY DEFINER helpers) on 100k rows. (Supabase RLS-perf; pgsql-performance list; GaryAustin1/RLS-Performance.)
- **#2 — index policy columns:** index every column a policy filters on (tenant_id, user_id/owner). ~171ms→<0.1ms on 100k rows. (Supabase.)
- **Plan-caching caveat:** prepared-statement generic plans (`plan_cache_mode=auto` after 5 execs) can't use the parameter value for selectivity → can plan RLS-on-parameter filters worse than a custom plan. Lever: `force_custom_plan` for highly selective per-tenant filters. (PG `sql-prepare`.)

### Security & correctness
- **THE footgun (most important for us):** superusers and `BYPASSRLS` roles **always** bypass RLS; table owners bypass it **unless** `FORCE ROW LEVEL SECURITY`. **`FORCE` does NOT rescue a superuser-run query — superuser bypass is unconditional.** So on our replica (tables owned by the `postgres` superuser) **every read MUST run under the non-superuser JWT role**; running as owner/superuser = silent, total bypass, no error. (PG `ddl-rowsecurity`, `sql-altertable`; Bytebase.)
- **Leakproof / side-channel:** policy filters run before user-query quals to avoid leaking hidden rows via errors/timing; only `LEAKPROOF` funcs may run earlier. `LEAKPROOF` is superuser-only and a security decision — don't mark helpers leakproof unless they truly can't leak. (PG docs; pganalyze.)
- **SECURITY DEFINER / search_path:** a definer function owned by a superuser effectively gains `BYPASSRLS` — never put it in an exposed schema. Pin an explicit `search_path` with `pg_temp` **last**; schema-qualifying tables alone is insufficient (operators resolve via search_path). (Supabase; Cybertec; PG `create-function`.)

### Helper functions & replication
- **Auth helpers** (`auth.uid()`/`auth.jwt()`): mark STABLE, read `current_setting('request.jwt.claims', true)`, reference as `(select …)` in policies. Pure claim-extraction needs no SECURITY DEFINER. (Supabase.)
- **Replication constraint:** logical replication propagates **neither DDL nor roles**. RLS policies, `ENABLE/FORCE`, and the non-superuser roles must be created **out-of-band** on the replica. (PG `logical-replication`.) ← this is exactly what `reconcile_security` does.
- **Apply privilege model (PG15+/16+):** apply runs as subscription owner; RLS is a hard *gate* during apply (superuser/owner/BYPASSRLS write through), **not** row-checked. Distinct from the read-time RLS path. pglite is PG17.5. (PG15 release notes / security docs.)

## Implications for our build (the deltas)

**Validated — already correct:**
1. **Non-superuser `query_as` + FORCE-as-hardening** = the research's #1 finding, verbatim. Our load-bearing decision is the right one.
2. **`reconcile_security` (introspect→rebuild policies/roles/grants out-of-band)** = exactly the required answer to "logical replication carries no DDL/roles."
3. **Replica replicates indexes** → policy-column indexing best practice inherited (if upstream indexes them).
4. **Apply-as-superuser writes** are correct — RLS only gates the read path; replication writing all rows is intended.

**Deltas to act on:**
1. **[replica-access-control] Helper-function gap.** Policies referencing functions (`auth.uid()`, custom helpers) need those functions to EXIST on the replica — replication/`reconcile_security` doesn't carry them, so `CREATE POLICY` fails-closed (halt). v1 options: (a) require upstream policies to be **self-contained** — inline `current_setting('request.jwt.claims', true)::json->>'…'`, no helper dependency; or (b) replicate referenced functions verbatim (keeping their pinned `search_path`) — v2. For midwess (plain PG, not Supabase), (a) is the simplest path; document it.
2. **[policy-authoring guidance]** Document for the upstream DBA: wrap `current_setting()` in `(select …)`; index tenant_id/owner columns; use a RESTRICTIVE tenant policy + permissive per-role SELECT; default-deny. The replica replicates policies verbatim, so it inherits whatever (good or bad) shape upstream writes.
3. **[validation — HIGHEST RISK]** The research's #1 open question is identical to our standing caveat: **does pglite actually enforce `SET LOCAL ROLE` + per-role RLS, and never run implicitly as superuser?** Unverified. This must be proven empirically (the gated `replica_access_control` test against a live upstream) **before** building PgPaw on top.

## Open questions (from the report)
- pglite faithfulness to `SET LOCAL ROLE` + RLS (highest risk; unverified).
- `plan_cache_mode` behavior on our per-request path (likely moot — we parse per query, no long-lived prepared statements).
- FORCE RLS: pure defense-in-depth here (no non-superuser owns replica tables); harmless to set.
- Column-privilege layering + keeping it in sync on the replica (ties to the deferred column-grant v2).

Full cited findings + sources: deep-research task `we2tat7r9`.
