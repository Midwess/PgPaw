"use client"

import { createCollection } from "@tanstack/db"
import { pgpawCollectionOptions } from "@pgpaw/tanstack-db"

export type BoardTodo = {
  id: string
  title: string
  completed: boolean
  project_id: string
  project: string
  assignee: string | null
}

export type Project = {
  id: string
  name: string
}

const url = process.env.NEXT_PUBLIC_PGPAW_URL!

async function txidOf(response: Response): Promise<{ txid: number }> {
  const { txid } = await response.json()
  return { txid }
}

export const boardCollection = createCollection(
  pgpawCollectionOptions<BoardTodo>({
    url,
    sql: `select t.id, t.title, t.completed, t.project_id,
                 p.name as project, u.name as assignee
          from todos t
          join projects p on p.id = t.project_id
          left join users u on u.id = t.assignee_id`,
    getKey: (row) => row.id,

    onInsert: async ({ transaction }) => {
      const row = transaction.mutations[0].modified
      return txidOf(
        await fetch("/api/todos", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ id: row.id, title: row.title, project_id: row.project_id }),
        }),
      )
    },

    onUpdate: async ({ transaction }) => {
      const { original, changes } = transaction.mutations[0]
      return txidOf(
        await fetch(`/api/todos/${original.id}`, {
          method: "PATCH",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ title: changes.title, completed: changes.completed }),
        }),
      )
    },

    onDelete: async ({ transaction }) => {
      const { original } = transaction.mutations[0]
      return txidOf(await fetch(`/api/todos/${original.id}`, { method: "DELETE" }))
    },
  }),
)

export const projectCollection = createCollection(
  pgpawCollectionOptions<Project>({
    url,
    sql: "select id, name from projects",
    getKey: (project) => project.id,

    onUpdate: async ({ transaction }) => {
      const { original, changes } = transaction.mutations[0]
      return txidOf(
        await fetch(`/api/projects/${original.id}`, {
          method: "PATCH",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ name: changes.name }),
        }),
      )
    },
  }),
)
