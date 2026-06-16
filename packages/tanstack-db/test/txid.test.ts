import { describe, expect, it } from "vitest"

import { TxidTracker } from "../src/txid"

describe("TxidTracker", () => {
  it("records and matches on the low 32 bits", () => {
    const tracker = new TxidTracker()
    tracker.record(0x1_0000_0007)
    expect(tracker.has(7)).toBe(true)
  })

  it("resolves immediately when the txid is already seen", async () => {
    const tracker = new TxidTracker()
    tracker.record(42)
    await expect(tracker.awaitTxId(42)).resolves.toBeUndefined()
  })

  it("resolves when the txid arrives later", async () => {
    const tracker = new TxidTracker()
    const pending = tracker.awaitTxId(99)
    tracker.record(99)
    await expect(pending).resolves.toBeUndefined()
  })

  it("rejects on timeout when the txid never arrives", async () => {
    const tracker = new TxidTracker()
    await expect(tracker.awaitTxId(1, 10)).rejects.toThrow(/timed out/)
  })
})
