import type { ReactNode } from "react"

export const metadata = {
  title: "PgPaw + TanStack DB — Todos",
}

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en">
      <body style={{ fontFamily: "system-ui, sans-serif", maxWidth: 520, margin: "3rem auto" }}>
        {children}
      </body>
    </html>
  )
}
