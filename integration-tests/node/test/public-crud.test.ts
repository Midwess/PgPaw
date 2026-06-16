import { describe, expect, it } from "vitest"

import { collectionFor, pgExec, waitFor } from "../harness/stack"

type Item = { id: string; name: string; n: number }

describe("public single-table collection", () => {
  it("preloads a snapshot then live-syncs insert / update / delete", async () => {
    await pgExec("delete from items")
    await pgExec("insert into items (id,name,n) values ('a','Alpha',1),('b','Beta',2)")

    const items = collectionFor<Item>({ sql: "select id, name, n from items", getKey: (r) => r.id })
    await items.preload()
    await waitFor(() => items.size, (s) => s === 2, { label: "snapshot=2" })
    expect(items.get("a")?.name).toBe("Alpha")

    await pgExec("insert into items (id,name,n) values ('c','Gamma',3)")
    await waitFor(() => items.get("c")?.name, (v) => v === "Gamma", { label: "live insert" })

    await pgExec("update items set name='Gamma2' where id='c'")
    await waitFor(() => items.get("c")?.name, (v) => v === "Gamma2", { label: "live update" })

    await pgExec("delete from items where id='c'")
    await waitFor(() => items.has("c"), (h) => h === false, { label: "live delete" })

    expect(items.size).toBe(2)
  })
})
