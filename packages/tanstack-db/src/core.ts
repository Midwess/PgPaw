import type {
  DeleteMutationFnParams,
  InsertMutationFnParams,
  SyncConfig,
  UpdateMutationFnParams,
} from "@tanstack/db"

import { TxidTracker } from "./txid"

export { TxidTracker } from "./txid"

type Row = Record<string, any>

export type WriteResult = { txid?: number } | void

export type SyncFrame<T extends Row> =
  | { kind: "snapshot"; rows: T[]; txid?: number }
  | { kind: "change"; op: "insert" | "update" | "delete"; row?: Partial<T>; key?: string; txid?: number }
  | { kind: "up-to-date"; txid?: number }
  | { kind: "reset"; txid?: number }

export interface DriverCoreConfig<T extends Row> {
  getKey: (row: T) => string | number
  source: (signal: AbortSignal) => AsyncIterable<SyncFrame<T>>
  rowUpdateMode: "partial" | "full"
  reconnectMs?: number
  normalizeTxid?: (txid: number) => string
  onInsert?: (params: InsertMutationFnParams<T, string>) => Promise<WriteResult>
  onUpdate?: (params: UpdateMutationFnParams<T, string>) => Promise<WriteResult>
  onDelete?: (params: DeleteMutationFnParams<T, string>) => Promise<WriteResult>
}

export function collectionOptionsFromSource<T extends Row>(config: DriverCoreConfig<T>) {
  const txids = new TxidTracker(config.normalizeTxid)
  const reconnectMs = config.reconnectMs ?? 1000

  const sync: SyncConfig<T, string>["sync"] = ({ begin, write, commit, markReady, truncate }) => {
    let stopped = false
    let current: AbortController | null = null
    let signalledReady = false
    const seen = new Set<string>()
    const keyOf = (row: Partial<T>) => String(config.getKey(row as T))

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

      const pending = new Map<string, { type: "upsert"; value: Partial<T> } | { type: "delete" }>()
      const flush = (): void => {
        if (pending.size === 0) return
        begin()
        for (const [key, op] of pending) {
          if (op.type === "upsert") {
            write({ type: seen.has(key) ? "update" : "insert", value: op.value as T })
            seen.add(key)
          } else if (seen.has(key)) {
            write({ type: "delete", key })
            seen.delete(key)
          }
        }
        commit()
        pending.clear()
      }

      for await (const frame of config.source(controller.signal)) {
        if (typeof frame.txid === "number") txids.record(frame.txid)

        if (frame.kind === "snapshot") {
          applyRows(frame.rows)
          ready()
          continue
        }
        if (frame.kind === "up-to-date") {
          flush()
          continue
        }
        if (frame.kind === "reset") {
          pending.clear()
          seen.clear()
          begin()
          truncate()
          commit()
          return
        }

        switch (frame.op) {
          case "insert":
          case "update": {
            const key = frame.row !== undefined ? keyOf(frame.row) : String(frame.key)
            const previous = pending.get(key)
            const merged =
              previous?.type === "upsert"
                ? { ...previous.value, ...(frame.row ?? {}) }
                : (frame.row ?? {})
            pending.set(key, { type: "upsert", value: merged })
            break
          }
          case "delete": {
            const key = frame.row !== undefined ? keyOf(frame.row) : String(frame.key)
            if (pending.get(key)?.type !== "upsert") pending.set(key, { type: "delete" })
            break
          }
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
    sync: { sync, rowUpdateMode: config.rowUpdateMode },
    onInsert: confirm(config.onInsert),
    onUpdate: confirm(config.onUpdate),
    onDelete: confirm(config.onDelete),
    utils: {
      awaitTxId: (txid: number, timeoutMs?: number) => txids.awaitTxId(txid, timeoutMs),
    },
  }
}
