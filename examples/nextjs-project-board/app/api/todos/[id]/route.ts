import { mutateReturningTxid } from "../../../../lib/db"

export async function PATCH(
  request: Request,
  { params }: { params: Promise<{ id: string }> },
) {
  const { id } = await params
  const { title, completed } = await request.json()
  const txid = await mutateReturningTxid(async (client) => {
    await client.query(
      "update todos set title = coalesce($2, title), completed = coalesce($3, completed) where id = $1",
      [id, title ?? null, completed ?? null],
    )
  })
  return Response.json({ txid })
}

export async function DELETE(
  _request: Request,
  { params }: { params: Promise<{ id: string }> },
) {
  const { id } = await params
  const txid = await mutateReturningTxid(async (client) => {
    await client.query("delete from todos where id = $1", [id])
  })
  return Response.json({ txid })
}
