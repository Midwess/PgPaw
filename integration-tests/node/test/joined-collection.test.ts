import { describe, expect, it } from "vitest"

import { collectionFor, pgExec, waitFor } from "../harness/stack"

type Board = {
  id: string
  title: string
  completed: boolean
  project_id: string
  project: string
  assignee: string | null
}

const SQL = `select t.id, t.title, t.completed, t.project_id, p.name as project, u.name as assignee
             from todos t
             join projects p on p.id = t.project_id
             left join users u on u.id = t.assignee_id`

describe("one collection over a 3-table join", () => {
  it("live-syncs the join, propagates a parent rename, and reflects row updates", async () => {
    await pgExec("delete from todos")
    await pgExec("update projects set name='Launch' where id='p1'")
    await pgExec(
      "insert into todos (id,title,completed,project_id,assignee_id) values ('tk1','task one',false,'p1','u1')",
    )

    const board = collectionFor<Board>({ sql: SQL, getKey: (r) => r.id })
    await board.preload()
    await waitFor(() => board.get("tk1")?.project, (v) => v === "Launch", { label: "join snapshot" })
    expect(board.get("tk1")?.assignee).toBe("Ada")

    await pgExec("update projects set name='Launchpad' where id='p1'")
    await waitFor(() => board.get("tk1")?.project, (v) => v === "Launchpad", {
      label: "cross-table rename propagation",
    })

    await pgExec("update todos set completed=true where id='tk1'")
    await waitFor(() => board.get("tk1")?.completed, (v) => v === true, { label: "join row update" })

    await pgExec("insert into todos (id,title,completed,project_id,assignee_id) values ('tk2','task two',false,'p2',null)")
    await waitFor(() => board.get("tk2")?.project, (v) => v === "Backlog", { label: "join live insert" })
    expect(board.get("tk2")?.assignee).toBeNull()
  })
})
