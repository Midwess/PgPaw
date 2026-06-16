import { mutateReturningTxid } from "../../../../lib/db"

export async function PATCH(
  request: Request,
  { params }: { params: Promise<{ id: string }> },
) {
  const { id } = await params
  const { name } = await request.json()
  const txid = await mutateReturningTxid(async (client) => {
    await client.query("update projects set name = $2 where id = $1", [id, name])
  })
  return Response.json({ txid })
}
