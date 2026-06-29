import type {
  DeleteMutationFnParams,
  InsertMutationFnParams,
  SyncConfig,
  UpdateMutationFnParams,
} from "@tanstack/db"

import { readSse } from "./stream"
import { TxidTracker } from "./txid"

export { TxidTracker } from "./txid"
export { readSse } from "./stream"

type Row = Record<string, any>

type WriteResult = { txid?: number } | void

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
  const txids = new TxidTracker()
  const reconnectMs = config.reconnectMs ?? 1000

  const resolveHeaders = async (): Promise<Record<string, string>> => {
    const extra = typeof config.headers === "function" ? await config.headers() : config.headers
    return { "content-type": "application/json", ...(extra ?? {}) }
  }

  const sync: SyncConfig<T, string>["sync"] = ({ begin, write, commit, markReady, truncate }) => {
    let stopped = false
    let current: AbortController | null = null
    let signalledReady = false
    const seen = new Set<string>()
    const keyOf = (row: T) => String(config.getKey(row))

    const ready = (): void => {
      if (!signalledReady) {
        signalledReady = true
        markReady()
      }
    }

    const applyRows = (rows: ReadonlyArray<T>): void => {
      seen.clear()
      begin()
      for (const row of rows) {
        write({ type: "insert", value: row })
        seen.add(keyOf(row))
      }
      commit()
    }

    const connect = async (): Promise<void> => {
      const controller = new AbortController()
      current = controller
      const response = await fetch(`${config.url}/query?live=true`, {
        method: "POST",
        headers: await resolveHeaders(),
        body: JSON.stringify({ sql: config.sql }),
        signal: controller.signal,
      })
      if (!response.ok) throw new Error(`PgPaw live request failed: ${response.status}`)

      const pending = new Map<string, { type: "upsert"; value: T } | { type: "delete" }>()
      const flush = (): void => {
        if (pending.size === 0) return
        begin()
        for (const [key, op] of pending) {
          if (op.type === "upsert") {
            write({ type: seen.has(key) ? "update" : "insert", value: op.value })
            seen.add(key)
          } else if (seen.has(key)) {
            write({ type: "delete", key })
            seen.delete(key)
          }
        }
        commit()
        pending.clear()
      }

      for await (const event of readSse(response)) {
        if (typeof event.txid === "number") txids.record(event.txid)

        if (event.type === "snapshot") {
          let rows = event.rows as T[] | undefined
          if (event.url) {
            const snapshot = await fetch(`${config.url}${event.url}`, {
              headers: await resolveHeaders(),
              signal: controller.signal,
            })
            rows = (await snapshot.json()) as T[]
          }
          applyRows(rows ?? [])
          ready()
          continue
        }

        switch (event.op) {
          case "insert":
          case "update": {
            const key = keyOf(event.row as T)
            pending.set(key, { type: "upsert", value: event.row as T })
            break
          }
          case "delete": {
            const key = event.row !== undefined ? keyOf(event.row as T) : String(event.key)
            if (pending.get(key)?.type !== "upsert") pending.set(key, { type: "delete" })
            break
          }
          case "up-to-date":
            flush()
            break
          case "reset":
            pending.clear()
            seen.clear()
            begin()
            truncate()
            commit()
            return
          default:
            break
        }
      }
    }

    const run = async (): Promise<void> => {
      while (!stopped) {
        try {
          await connect()
        } catch {
          ready()
        } finally {
          current?.abort()
        }
        if (stopped) break
        await new Promise((wake) => setTimeout(wake, reconnectMs))
      }
    }

    void run()
    return () => {
      stopped = true
      current?.abort()
    }
  }

  const confirm = <P>(handler?: (params: P) => Promise<WriteResult>) =>
    handler
      ? async (params: P): Promise<WriteResult> => {
          const result = await handler(params)
          if (result && typeof result.txid === "number") await txids.awaitTxId(result.txid)
          return result
        }
      : undefined

  return {
    getKey: (row: T) => String(config.getKey(row)),
    sync: { sync, rowUpdateMode: "full" as const },
    onInsert: confirm(config.onInsert),
    onUpdate: confirm(config.onUpdate),
    onDelete: confirm(config.onDelete),
    utils: {
      awaitTxId: (txid: number, timeoutMs?: number) => txids.awaitTxId(txid, timeoutMs),
    },
  }
}
