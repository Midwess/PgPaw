import { writeReturningTxid } from "../../../lib/db"

export async function POST(request: Request) {
  const { id, title, completed } = await request.json()
  const txid = await writeReturningTxid(
    `with ins as (
       insert into todos (id, title, completed) values ($1, $2, $3)
     )
     select pg_current_xact_id()::xid::text as txid`,
    [id, title, completed ?? false],
  )
  return Response.json({ txid })
}
