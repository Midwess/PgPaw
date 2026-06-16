"use client"

import dynamic from "next/dynamic"

const TodoList = dynamic(() => import("./todo-list"), { ssr: false })

export default function Page() {
  return <TodoList />
}
