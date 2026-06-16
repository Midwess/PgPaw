import { describe, expect, it } from "vitest"

import { readSse } from "../src/stream"

function sseResponse(chunks: string[]): Response {
  const encoder = new TextEncoder()
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      for (const chunk of chunks) controller.enqueue(encoder.encode(chunk))
      controller.close()
    },
  })
  return new Response(stream, { status: 200 })
}

describe("readSse", () => {
  it("parses data frames split across chunk boundaries", async () => {
    const response = sseResponse(['data: {"a":1}\n', '\ndata: {"b":2}\n\n'])
    const events: Array<Record<string, unknown>> = []
    for await (const event of readSse(response)) events.push(event)
    expect(events).toEqual([{ a: 1 }, { b: 2 }])
  })

  it("skips keep-alive comment frames", async () => {
    const response = sseResponse([": keep-alive\n\n", 'data: {"ok":true}\n\n'])
    const events: Array<Record<string, unknown>> = []
    for await (const event of readSse(response)) events.push(event)
    expect(events).toEqual([{ ok: true }])
  })
})
