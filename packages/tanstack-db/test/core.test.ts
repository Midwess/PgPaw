import { describe, expect, it } from "vitest"

import type { SyncFrame } from "../src/core"
import { collectionOptionsFromSource, TxidTracker } from "../src/core"

const tick = (ms = 20): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms))

type Row = { id: string; title?: string; done?: boolean }

function driver() {
  const events: Array<{ op: string; payload?: unknown }> = []
  const handlers = {
    begin: () => events.push({ op: "begin" }),
    write: (message: unknown) => events.push({ op: "write", payload: message }),
    commit: () => events.push({ op: "commit" }),
    markReady: () => events.push({ op: "ready" }),
    truncate: () => events.push({ op: "truncate" }),
  }
  return { events, handlers }
}

function queueSource(batches: SyncFrame<Row>[][]) {
  let calls = 0
  const source = (_signal: AbortSignal): AsyncIterable<SyncFrame<Row>> => {
    const batch = batches[Math.min(calls, batches.length - 1)] ?? []
    calls += 1
    return (async function* () {
      for (const frame of batch) yield frame
      await new Promise(() => {})
    })()
  }
  return { source, count: () => calls }
}

describe("collectionOptionsFromSource", () => {
  it("streams snapshot then merges partial patches per commit batch", async () => {
    const { source } = queueSource([
      [
        { kind: "snapshot", rows: [{ id: "t1", title: "first", done: false }] },
        { kind: "change", op: "update", row: { id: "t1", title: "renamed" }, key: "t1", txid: 7 },
        { kind: "change", op: "update", row: { id: "t1", done: true }, key: "t1", txid: 7 },
        { kind: "up-to-date" },
      ],
    ])
    const options = collectionOptionsFromSource<Row>({
      getKey: (row) => row.id,
      source,
      rowUpdateMode: "partial",
      reconnectMs: 5,
    })
    expect(options.sync.rowUpdateMode).toBe("partial")
    const { events, handlers } = driver()
    const stop = options.sync.sync(handlers as never) as () => void
    await tick()

    expect(events.filter((e) => e.op === "ready")).toHaveLength(1)
    expect(events).toContainEqual({
      op: "write",
      payload: { type: "insert", value: { id: "t1", title: "first", done: false } },
    })
    expect(events).toContainEqual({
      op: "write",
      payload: { type: "update", value: { id: "t1", title: "renamed", done: true } },
    })
    await expect(options.utils.awaitTxId(7)).resolves.toBeUndefined()
    stop()
  })

  it("resets on a reset frame and reconnects with a fresh source", async () => {
    const queue = queueSource([
      [{ kind: "snapshot", rows: [{ id: "a" }] }, { kind: "reset" }],
      [{ kind: "snapshot", rows: [{ id: "b" }] }],
    ])
    const options = collectionOptionsFromSource<Row>({
      getKey: (row) => row.id,
      source: queue.source,
      rowUpdateMode: "full",
      reconnectMs: 5,
    })
    const { events, handlers } = driver()
    const stop = options.sync.sync(handlers as never) as () => void
    await tick(40)

    const resetSlice = events.slice(events.findIndex((e) => e.op === "truncate") - 1)
    expect(resetSlice.slice(0, 3).map((e) => e.op)).toEqual(["begin", "truncate", "commit"])
    expect(queue.count()).toBeGreaterThanOrEqual(2)
    expect(events).toContainEqual({ op: "write", payload: { type: "insert", value: { id: "b" } } })
    stop()
  })

  it("lets a row-less delete win over an earlier upsert in the same batch", async () => {
    const { source } = queueSource([
      [
        { kind: "snapshot", rows: [{ id: "t1" }] },
        { kind: "change", op: "update", row: { id: "t1", done: true }, key: "t1", txid: 7 },
        { kind: "change", op: "delete", key: "t1", txid: 8 },
        { kind: "up-to-date" },
      ],
    ])
    const options = collectionOptionsFromSource<Row>({
      getKey: (row) => row.id,
      source,
      rowUpdateMode: "partial",
      reconnectMs: 5,
    })
    const { events, handlers } = driver()
    const stop = options.sync.sync(handlers as never) as () => void
    await tick()

    expect(events).toContainEqual({ op: "write", payload: { type: "delete", key: "t1" } })
    expect(events).not.toContainEqual({
      op: "write",
      payload: { type: "update", value: { id: "t1", done: true } },
    })
    stop()
  })

  it("awaits a returned txid before resolving a write handler", async () => {
    const { source } = queueSource([
      [
        { kind: "snapshot", rows: [] },
        { kind: "up-to-date", txid: 99 },
      ],
    ])
    const options = collectionOptionsFromSource<Row>({
      getKey: (row) => row.id,
      source,
      rowUpdateMode: "partial",
      reconnectMs: 5,
      onInsert: async () => ({ txid: 99 }),
    })
    const { handlers } = driver()
    const stop = options.sync.sync(handlers as never) as () => void
    await tick()
    await expect(options.onInsert!({} as never)).resolves.toEqual({ txid: 99 })
    stop()
  })
})

describe("TxidTracker normalization", () => {
  it("defaults to low-32 matching", () => {
    const tracker = new TxidTracker()
    tracker.record(0x1_0000_0007)
    expect(tracker.has(7)).toBe(true)
  })

  it("keeps full width when normalize is String", () => {
    const tracker = new TxidTracker((txid) => String(txid))
    tracker.record(2 ** 32 + 7)
    expect(tracker.has(7)).toBe(false)
    expect(tracker.has(2 ** 32 + 7)).toBe(true)
  })
})
