import type {
  DeleteMutationFnParams,
  InsertMutationFnParams,
  UpdateMutationFnParams,
} from "@tanstack/db"

import type { SyncFrame, WriteResult } from "./core"
import { collectionOptionsFromSource } from "./core"
import { readSse } from "./stream"

export { TxidTracker } from "./txid"
export { readSse } from "./stream"
export { collectionOptionsFromSource } from "./core"
export type { DriverCoreConfig, SyncFrame, WriteResult } from "./core"

type Row = Record<string, any>

export interface PgpawCollectionConfig<T extends Row> {
  url: string
  sql: string
  getKey: (row: T) => string | number
  headers?:
    | Record<string, string>
    | (() => Record<string, string> | Promise<Record<string, string>>)
  reconnectMs?: number
  onInsert?: (params: InsertMutationFnParams<T, string>) => Promise<WriteResult>
  onUpdate?: (params: UpdateMutationFnParams<T, string>) => Promise<WriteResult>
  onDelete?: (params: DeleteMutationFnParams<T, string>) => Promise<WriteResult>
}

export function pgpawCollectionOptions<T extends Row>(config: PgpawCollectionConfig<T>) {
  const resolveHeaders = async (): Promise<Record<string, string>> => {
    const extra = typeof config.headers === "function" ? await config.headers() : config.headers
    return { "content-type": "application/json", ...(extra ?? {}) }
  }

  const source = (signal: AbortSignal): AsyncIterable<SyncFrame<T>> =>
    (async function* (): AsyncGenerator<SyncFrame<T>> {
      const response = await fetch(`${config.url}/query?live=true`, {
        method: "POST",
        headers: await resolveHeaders(),
        body: JSON.stringify({ sql: config.sql }),
        signal,
      })
      if (!response.ok) throw new Error(`PgPaw live request failed: ${response.status}`)

      for await (const event of readSse(response)) {
        const txid = typeof event.txid === "number" ? event.txid : undefined

        if (event.type === "snapshot") {
          let rows = event.rows as T[] | undefined
          if (event.url) {
            const snapshot = await fetch(`${config.url}${event.url}`, {
              headers: await resolveHeaders(),
              signal,
            })
            rows = (await snapshot.json()) as T[]
          }
          yield { kind: "snapshot", rows: rows ?? [], txid }
          continue
        }

        switch (event.op) {
          case "insert":
          case "update":
          case "delete":
            yield {
              kind: "change",
              op: event.op,
              row: event.row as T | undefined,
              key: event.key !== undefined ? String(event.key) : undefined,
              txid,
            }
            break
          case "up-to-date":
            yield { kind: "up-to-date", txid }
            break
          case "reset":
            yield { kind: "reset", txid }
            break
          default:
            break
        }
      }
    })()

  return collectionOptionsFromSource<T>({
    getKey: config.getKey,
    source,
    rowUpdateMode: "full",
    reconnectMs: config.reconnectMs,
    onInsert: config.onInsert,
    onUpdate: config.onUpdate,
    onDelete: config.onDelete,
  })
}
