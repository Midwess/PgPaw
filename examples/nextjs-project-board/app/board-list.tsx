"use client"

import { useState } from "react"
import { useLiveQuery } from "@tanstack/react-db"

import { boardCollection, projectCollection, type BoardTodo } from "../lib/board"

export default function BoardList() {
  const { data: todos } = useLiveQuery((q) =>
    q.from({ todo: boardCollection }).select(({ todo }) => ({
      id: todo.id,
      title: todo.title,
      completed: todo.completed,
      project: todo.project,
      assignee: todo.assignee,
    })),
  )
  const { data: projects } = useLiveQuery((q) =>
    q.from({ project: projectCollection }).select(({ project }) => ({
      id: project.id,
      name: project.name,
    })),
  )

  const [title, setTitle] = useState("")
  const [projectId, setProjectId] = useState("")

  const activeProject = projectId || projects[0]?.id
  const nameOf = (id: string) => projects.find((p) => p.id === id)?.name ?? ""

  const add = () => {
    const value = title.trim()
    if (!value || !activeProject) return
    boardCollection.insert({
      id: crypto.randomUUID(),
      title: value,
      completed: false,
      project_id: activeProject,
      project: nameOf(activeProject),
      assignee: null,
    })
    setTitle("")
  }

  const toggle = (todo: BoardTodo) =>
    boardCollection.update(todo.id, (draft) => {
      draft.completed = !draft.completed
    })

  const remove = (todo: BoardTodo) => boardCollection.delete(todo.id)

  const rename = (id: string, current: string) => {
    const next = window.prompt("Rename project", current)
    if (next && next !== current)
      projectCollection.update(id, (draft) => {
        draft.name = next
      })
  }

  const sorted = [...todos].sort(
    (a, b) => a.project.localeCompare(b.project) || a.title.localeCompare(b.title),
  )

  return (
    <main>
      <h1>Project board</h1>
      <p style={{ color: "#666" }}>
        One collection over a 3-table join (todos ⋈ projects ⋈ users). Rename a
        project and every row updates live.
      </p>

      <section style={{ display: "flex", gap: 8, flexWrap: "wrap", margin: "1rem 0" }}>
        {projects.map((project) => (
          <button key={project.id} onClick={() => rename(project.id, project.name)}>
            {project.name} ✎
          </button>
        ))}
      </section>

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
          placeholder="New task"
          style={{ flex: 1 }}
        />
        <select value={activeProject ?? ""} onChange={(event) => setProjectId(event.target.value)}>
          {projects.map((project) => (
            <option key={project.id} value={project.id}>
              {project.name}
            </option>
          ))}
        </select>
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
            <span style={{ fontSize: 12, background: "#eee", borderRadius: 4, padding: "2px 6px" }}>
              {todo.project}
            </span>
            {todo.assignee ? (
              <span style={{ fontSize: 12, color: "#666" }}>@{todo.assignee}</span>
            ) : null}
            <button onClick={() => remove(todo)} aria-label={`Delete ${todo.title}`}>
              ✕
            </button>
          </li>
        ))}
      </ul>
    </main>
  )
}
