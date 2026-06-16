"use client"

import dynamic from "next/dynamic"

const BoardList = dynamic(() => import("./board-list"), { ssr: false })

export default function Page() {
  return <BoardList />
}
