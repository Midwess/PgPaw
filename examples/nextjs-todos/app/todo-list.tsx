"use client"

import { useState } from "react"
import { useLiveQuery } from "@tanstack/react-db"

import { todoCollection, type Todo } from "../lib/todos"

export default function TodoList() {
  const { data: todos } = useLiveQuery((q) =>
    q.from({ todo: todoCollection }).select(({ todo }) => ({
      id: todo.id,
      title: todo.title,
      completed: todo.completed,
    })),
  )
  const [title, setTitle] = useState("")

  const add = () => {
    const value = title.trim()
    if (!value) return
    todoCollection.insert({ id: crypto.randomUUID(), title: value, completed: false })
    setTitle("")
  }

  const toggle = (todo: Todo) =>
    todoCollection.update(todo.id, (draft) => {
      draft.completed = !draft.completed
    })

  const remove = (todo: Todo) => todoCollection.delete(todo.id)

  const sorted = [...todos].sort((a, b) => a.title.localeCompare(b.title))

  return (
    <main>
      <h1>Todos</h1>
      <form
        onSubmit={(event) => {
          event.preventDefault()
          add()
        }}
        style={{ display: "flex", gap: 8 }}
      >
        <input
          value={title}
          onChange={(event) => setTitle(event.target.value)}
          placeholder="What needs doing?"
          style={{ flex: 1 }}
        />
        <button type="submit">Add</button>
      </form>

      <ul style={{ listStyle: "none", padding: 0 }}>
        {sorted.map((todo) => (
          <li
            key={todo.id}
            style={{ display: "flex", gap: 8, alignItems: "center", padding: "4px 0" }}
          >
            <input type="checkbox" checked={todo.completed} onChange={() => toggle(todo)} />
            <span style={{ flex: 1, textDecoration: todo.completed ? "line-through" : "none" }}>
              {todo.title}
            </span>
            <button onClick={() => remove(todo)} aria-label={`Delete ${todo.title}`}>
              ✕
            </button>
          </li>
        ))}
      </ul>
    </main>
  )
}
