import { Pool, type PoolClient } from "pg"

const globalForPg = globalThis as unknown as { pgPool?: Pool }

export const pool =
  globalForPg.pgPool ?? new Pool({ connectionString: process.env.DATABASE_URL })

if (process.env.NODE_ENV !== "production") globalForPg.pgPool = pool

export async function mutateReturningTxid(
  write: (client: PoolClient) => Promise<void>,
): Promise<number> {
  const client = await pool.connect()
  try {
    await client.query("begin")
    await write(client)
    const { rows } = await client.query("select pg_current_xact_id()::xid::text as txid")
    await client.query("commit")
    return Number(rows[0].txid)
  } catch (error) {
    await client.query("rollback")
    throw error
  } finally {
    client.release()
  }
}
