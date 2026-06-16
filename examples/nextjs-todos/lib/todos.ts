"use client"

import { createCollection } from "@tanstack/db"
import { pgpawCollectionOptions } from "@pgpaw/tanstack-db"

export type Todo = {
  id: string
  title: string
  completed: boolean
}

async function txidOf(response: Response): Promise<{ txid: number }> {
  const { txid } = await response.json()
  return { txid }
}

export const todoCollection = createCollection(
  pgpawCollectionOptions<Todo>({
    url: process.env.NEXT_PUBLIC_PGPAW_URL!,
    sql: "select id, title, completed from todos",
    getKey: (todo) => todo.id,

    onInsert: async ({ transaction }) => {
      const todo = transaction.mutations[0].modified
      return txidOf(
        await fetch("/api/todos", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(todo),
        }),
      )
    },

    onUpdate: async ({ transaction }) => {
      const { original, changes } = transaction.mutations[0]
      return txidOf(
        await fetch(`/api/todos/${original.id}`, {
          method: "PATCH",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(changes),
        }),
      )
    },

    onDelete: async ({ transaction }) => {
      const { original } = transaction.mutations[0]
      return txidOf(await fetch(`/api/todos/${original.id}`, { method: "DELETE" }))
    },
  }),
)
