import { Pool } from "pg"

const globalForPg = globalThis as unknown as { pgPool?: Pool }

export const pool =
  globalForPg.pgPool ?? new Pool({ connectionString: process.env.DATABASE_URL })

if (process.env.NODE_ENV !== "production") globalForPg.pgPool = pool

export async function writeReturningTxid(
  sql: string,
  params: ReadonlyArray<unknown>,
): Promise<number> {
  const { rows } = await pool.query(sql, params as unknown[])
  return Number(rows[0].txid)
}
