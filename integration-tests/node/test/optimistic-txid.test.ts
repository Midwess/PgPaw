import { describe, expect, it } from "vitest"

import { collectionFor, pgExec, writeTxid } from "../harness/stack"

type Item = { id: string; name: string; n: number }

function itemsCollection() {
  return collectionFor<Item>({
    sql: "select id, name, n from items",
    getKey: (r) => r.id,
    onInsert: async ({ transaction }) => {
      const m = transaction.mutations[0].modified as Item
      const txid = await writeTxid(
        `insert into items (id,name,n) values ('${m.id}','${m.name}',${m.n})`,
      )
      return { txid }
    },
    onUpdate: async ({ transaction }) => {
      const { original, changes } = transaction.mutations[0]
      const txid = await writeTxid(`update items set name='${changes.name}' where id='${original.id}'`)
      return { txid }
    },
    onDelete: async ({ transaction }) => {
      const { original } = transaction.mutations[0]
      const txid = await writeTxid(`delete from items where id='${original.id}'`)
      return { txid }
    },
  })
}

describe("optimistic writes confirmed by transaction id", () => {
  it("insert / update / delete apply optimistically and persist after the txid round-trip", async () => {
    await pgExec("delete from items")
    const items = itemsCollection()
    await items.preload()

    const tx = items.insert({ id: "opt1", name: "Opt", n: 7 })
    expect(items.get("opt1")?.name).toBe("Opt")
    await tx.isPersisted.promise
    expect((await pgExec("select name from items where id='opt1'")).rows[0].name).toBe("Opt")
    expect(items.get("opt1")?.name).toBe("Opt")

    const txu = items.update("opt1", (d) => {
      d.name = "Opt2"
    })
    expect(items.get("opt1")?.name).toBe("Opt2")
    await txu.isPersisted.promise
    expect((await pgExec("select name from items where id='opt1'")).rows[0].name).toBe("Opt2")

    const txd = items.delete("opt1")
    expect(items.has("opt1")).toBe(false)
    await txd.isPersisted.promise
    expect((await pgExec("select count(*)::int c from items where id='opt1'")).rows[0].c).toBe(0)
  })

  it("awaitTxId rejects when the transaction id never arrives", async () => {
    const items = collectionFor<Item>({ sql: "select id, name, n from items", getKey: (r) => r.id })
    await items.preload()
    const utils = (items as unknown as { utils: { awaitTxId: (t: number, ms?: number) => Promise<void> } }).utils
    await expect(utils.awaitTxId(2_000_000_111, 800)).rejects.toThrow(/timed out/)
  })
})
