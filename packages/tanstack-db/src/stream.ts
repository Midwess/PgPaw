export async function* readSse(response: Response): AsyncGenerator<Record<string, any>> {
  const body = response.body
  if (!body) throw new Error("PgPaw live response has no body")
  const reader = body.getReader()
  const decoder = new TextDecoder()
  let buffer = ""
  try {
    while (true) {
      const { done, value } = await reader.read()
      if (done) break
      buffer += decoder.decode(value, { stream: true })
      let boundary = buffer.indexOf("\n\n")
      while (boundary !== -1) {
        const frame = buffer.slice(0, boundary)
        buffer = buffer.slice(boundary + 2)
        const event = parseFrame(frame)
        if (event !== null) yield event
        boundary = buffer.indexOf("\n\n")
      }
    }
  } finally {
    reader.releaseLock()
  }
}

function parseFrame(frame: string): Record<string, any> | null {
  const line = frame.split("\n").find((candidate) => candidate.startsWith("data:"))
  if (!line) return null
  const payload = line.slice(line.indexOf(":") + 1).trim()
  if (!payload) return null
  return JSON.parse(payload)
}
