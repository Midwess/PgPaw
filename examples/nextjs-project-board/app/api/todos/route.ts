import { mutateReturningTxid } from "../../../lib/db"

export async function POST(request: Request) {
  const { id, title, project_id } = await request.json()
  const txid = await mutateReturningTxid(async (client) => {
    await client.query("insert into todos (id, title, project_id) values ($1, $2, $3)", [
      id,
      title,
      project_id,
    ])
  })
  return Response.json({ txid })
}
