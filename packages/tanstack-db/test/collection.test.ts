import { afterEach, describe, expect, it, vi } from "vitest"

import { pgpawCollectionOptions } from "../src"

const encoder = new TextEncoder()
const tick = (ms = 20): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms))

function liveStream() {
  let controller!: ReadableStreamDefaultController<Uint8Array>
  const stream = new ReadableStream<Uint8Array>({
    start(value) {
      controller = value
    },
  })
  return { stream, push: (frame: string) => controller.enqueue(encoder.encode(frame)) }
}

function stubFetch(liveBody: ReadableStream<Uint8Array>, snapshotRows?: unknown) {
  let first = true
  const mock = vi.fn(async (input: unknown) => {
    const url = String(input)
    if (url.includes("live=true")) {
      if (first) {
        first = false
        return new Response(liveBody, { status: 200 })
      }
      return new Response(new ReadableStream<Uint8Array>(), { status: 200 })
    }
    return new Response(JSON.stringify(snapshotRows ?? []), { status: 200 })
  })
  vi.stubGlobal("fetch", mock)
  return mock
}

function driver() {
  const events: Array<{ op: string; payload?: unknown }> = []
  const handlers = {
    begin: () => events.push({ op: "begin" }),
    write: (message: unknown) => events.push({ op: "write", payload: message }),
    commit: () => events.push({ op: "commit" }),
    markReady: () => events.push({ op: "ready" }),
    truncate: () => events.push({ op: "truncate" }),
    collection: {} as never,
  }
  return { events, handlers }
}

afterEach(() => vi.unstubAllGlobals())

describe("pgpawCollectionOptions sync", () => {
  it("loads an inline snapshot, applies deltas, and records txids", async () => {
    const live = liveStream()
    stubFetch(live.stream)
    const options = pgpawCollectionOptions<{ id: number; v: string }>({
      url: "http://pgpaw",
      sql: "select * from t",
      getKey: (row) => row.id,
      reconnectMs: 5,
    })
    const { events, handlers } = driver()
    const stop = options.sync.sync(handlers as never) as () => void

    live.push('data: {"type":"snapshot","rows":[{"id":1,"v":"a"}],"version":1}\n\n')
    await tick()
    expect(events.filter((e) => e.op === "ready")).toHaveLength(1)
    expect(events).toContainEqual({ op: "write", payload: { type: "insert", value: { id: 1, v: "a" } } })

    live.push('data: {"op":"update","key":"1","row":{"id":1,"v":"b"},"txid":42}\n\n')
    await tick()
    expect(events).toContainEqual({ op: "write", payload: { type: "update", value: { id: 1, v: "b" } } })
    await expect(options.utils.awaitTxId(42)).resolves.toBeUndefined()

    live.push('data: {"op":"delete","key":"1","txid":43}\n\n')
    await tick()
    expect(events).toContainEqual({ op: "write", payload: { type: "delete", key: "1" } })

    stop()
  })

  it("fetches the snapshot pointer for public queries", async () => {
    const live = liveStream()
    const fetchMock = stubFetch(live.stream, [{ id: 9, v: "z" }])
    const options = pgpawCollectionOptions<{ id: number; v: string }>({
      url: "http://pgpaw",
      sql: "select * from t",
      getKey: (row) => row.id,
      reconnectMs: 5,
    })
    const { events, handlers } = driver()
    const stop = options.sync.sync(handlers as never) as () => void

    live.push('data: {"type":"snapshot","url":"/q/abc/1","version":1}\n\n')
    await tick()
    expect(fetchMock).toHaveBeenCalledWith("http://pgpaw/q/abc/1", expect.anything())
    expect(events).toContainEqual({ op: "write", payload: { type: "insert", value: { id: 9, v: "z" } } })

    stop()
  })

  it("truncates inside a transaction on reset", async () => {
    const live = liveStream()
    stubFetch(live.stream)
    const options = pgpawCollectionOptions({
      url: "http://pgpaw",
      sql: "select 1",
      getKey: (row: { id: number }) => row.id,
      reconnectMs: 5,
    })
    const { events, handlers } = driver()
    const stop = options.sync.sync(handlers as never) as () => void

    live.push('data: {"type":"snapshot","rows":[],"version":1}\n\n')
    await tick()
    live.push('data: {"op":"reset"}\n\n')
    await tick()

    const resetSlice = events.slice(events.findIndex((e) => e.op === "truncate") - 1)
    expect(resetSlice.map((e) => e.op)).toEqual(["begin", "truncate", "commit"])

    stop()
  })

  it("awaits the returned txid before resolving a write handler", async () => {
    const live = liveStream()
    stubFetch(live.stream)
    const options = pgpawCollectionOptions({
      url: "http://pgpaw",
      sql: "select 1",
      getKey: (row: { id: number }) => row.id,
      reconnectMs: 5,
      onInsert: async () => ({ txid: 99 }),
    })
    const { handlers } = driver()
    const stop = options.sync.sync(handlers as never) as () => void

    live.push('data: {"type":"snapshot","rows":[],"version":1}\n\n')
    await tick()

    let resolved = false
    const pending = options.onInsert!({} as never).then(() => {
      resolved = true
    })
    await tick()
    expect(resolved).toBe(false)

    live.push('data: {"op":"up-to-date","txid":99}\n\n')
    await tick()
    await pending
    expect(resolved).toBe(true)

    stop()
  })
})
