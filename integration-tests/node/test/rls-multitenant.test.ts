import { describe, expect, it } from "vitest"

import { PGPAW_URL } from "../harness/env"
import { collectionFor, mintJwt, pgExec, waitFor } from "../harness/stack"

type Doc = { id: number; org_id: number; title: string }

const SQL = "select id, org_id, title from documents order by id"
const tokenA = mintJwt({ role: "member", org_id: 1 })
const tokenB = mintJwt({ role: "member", org_id: 2 })

describe("RLS multi-tenant isolation (private live)", () => {
  it("each tenant sees only its own rows and live deltas stay scoped", async () => {
    await pgExec("delete from documents where id >= 900")

    const a = collectionFor<Doc>({ sql: SQL, getKey: (r) => r.id, token: tokenA })
    const b = collectionFor<Doc>({ sql: SQL, getKey: (r) => r.id, token: tokenB })
    await a.preload()
    await b.preload()

    await waitFor(() => a.size, (s) => s === 2, { label: "A=2" })
    await waitFor(() => b.size, (s) => s === 3, { label: "B=3" })
    expect([...a.values()].every((d) => d.org_id === 1)).toBe(true)
    expect([...b.values()].every((d) => d.org_id === 2)).toBe(true)

    await pgExec("insert into documents values (901,1,'A-live')")
    await waitFor(() => a.has("901"), (h) => h, { label: "A receives its live insert" })
    await new Promise((r) => setTimeout(r, 1500))
    expect(b.has("901")).toBe(false)
  })

  it("rejects an access-controlled query without a valid token", async () => {
    const body = JSON.stringify({ sql: SQL })
    const noToken = await fetch(`${PGPAW_URL}/query?live=true`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body,
    })
    expect(noToken.status).toBe(401)

    const expired = mintJwt({ role: "member", org_id: 1 }, 100)
    const withExpired = await fetch(`${PGPAW_URL}/query?live=true`, {
      method: "POST",
      headers: { "content-type": "application/json", authorization: `Bearer ${expired}` },
      body,
    })
    expect(withExpired.status).toBe(401)
  })
})
