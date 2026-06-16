import { createCollection, type Collection } from "@tanstack/db"
import { pgpawCollectionOptions } from "@pgpaw/tanstack-db"
import jwt from "jsonwebtoken"
import pg from "pg"

import { DB_URL, JWT_SECRET, PGPAW_URL } from "./env"

const FAR_EXP = 4_070_908_800

export function mintJwt(claims: Record<string, unknown>, exp: number = FAR_EXP): string {
  return jwt.sign({ exp, ...claims }, JWT_SECRET, { algorithm: "HS256" })
}

export async function pgExec(sql: string): Promise<pg.QueryResult> {
  const client = new pg.Client({ connectionString: DB_URL })
  await client.connect()
  try {
    return await client.query(sql)
  } finally {
    await client.end()
  }
}

export async function writeTxid(sql: string): Promise<number> {
  const client = new pg.Client({ connectionString: DB_URL })
  await client.connect()
  try {
    await client.query("begin")
    await client.query(sql)
    const { rows } = await client.query("select pg_current_xact_id()::xid::text as txid")
    await client.query("commit")
    return Number(rows[0].txid)
  } catch (error) {
    await client.query("rollback")
    throw error
  } finally {
    await client.end()
  }
}

type Handler = (params: { transaction: any }) => Promise<{ txid?: number } | void>

const liveCollections: Array<Collection<any, any>> = []

export function collectionFor<T extends Record<string, any>>(config: {
  sql: string
  getKey: (row: T) => string | number
  token?: string
  onInsert?: Handler
  onUpdate?: Handler
  onDelete?: Handler
}): Collection<T, string> {
  const collection = createCollection(
    pgpawCollectionOptions<T>({
      url: PGPAW_URL,
      sql: config.sql,
      getKey: config.getKey,
      headers: config.token ? { authorization: `Bearer ${config.token}` } : undefined,
      onInsert: config.onInsert,
      onUpdate: config.onUpdate,
      onDelete: config.onDelete,
    }),
  ) as unknown as Collection<T, string>
  liveCollections.push(collection)
  return collection
}

export async function cleanupCollections(): Promise<void> {
  await Promise.all(liveCollections.map((c) => c.cleanup().catch(() => {})))
  liveCollections.length = 0
}

export async function waitFor<T>(
  produce: () => T | Promise<T>,
  predicate: (value: T) => boolean,
  { timeout = 15000, interval = 150, label = "condition" }: { timeout?: number; interval?: number; label?: string } = {},
): Promise<T> {
  const start = Date.now()
  let last: T
  for (;;) {
    last = await produce()
    if (predicate(last)) return last
    if (Date.now() - start > timeout) {
      throw new Error(`waitFor(${label}) timed out after ${timeout}ms; last=${JSON.stringify(last)}`)
    }
    await new Promise((r) => setTimeout(r, interval))
  }
}

export const titles = (rows: Array<{ title?: string }>): string[] => rows.map((r) => r.title ?? "").sort()
export const ids = (rows: Array<{ id: number | string }>): Array<number | string> =>
  rows.map((r) => r.id).sort()
