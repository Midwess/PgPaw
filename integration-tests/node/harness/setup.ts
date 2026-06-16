import { afterEach } from "vitest"

import { cleanupCollections } from "./stack"

afterEach(async () => {
  await cleanupCollections()
})
