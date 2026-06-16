import { describe, expect, it } from "vitest"

import { PGPAW_URL } from "../harness/env"

async function statusOf(sql: string): Promise<number> {
  const res = await fetch(`${PGPAW_URL}/query`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ sql }),
    redirect: "manual",
  })
  return res.status
}

describe("read-only SQL classifier rejects unsafe queries", () => {
  it("accepts a read-only SELECT over a replicated table (303 redirect)", async () => {
    expect(await statusOf("select id, name from items")).toBe(303)
  })

  it("rejects writes, DDL, multi-statement, and non-replicated tables (400)", async () => {
    expect(await statusOf("update items set n = 1")).toBe(400)
    expect(await statusOf("delete from items")).toBe(400)
    expect(await statusOf("drop table items")).toBe(400)
    expect(await statusOf("select 1; select 2")).toBe(400)
    expect(await statusOf("select * from definitely_not_a_table")).toBe(400)
  })
})
