import { writeReturningTxid } from "../../../../lib/db"

export async function PATCH(
  request: Request,
  { params }: { params: Promise<{ id: string }> },
) {
  const { id } = await params
  const { title, completed } = await request.json()
  const txid = await writeReturningTxid(
    `with upd as (
       update todos set title = coalesce($2, title), completed = coalesce($3, completed)
       where id = $1
     )
     select pg_current_xact_id()::xid::text as txid`,
    [id, title ?? null, completed ?? null],
  )
  return Response.json({ txid })
}

export async function DELETE(
  _request: Request,
  { params }: { params: Promise<{ id: string }> },
) {
  const { id } = await params
  const txid = await writeReturningTxid(
    `with del as (delete from todos where id = $1)
     select pg_current_xact_id()::xid::text as txid`,
    [id],
  )
  return Response.json({ txid })
}
