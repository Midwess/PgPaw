import { defineConfig } from "vitest/config"

export default defineConfig({
  test: {
    globalSetup: ["./harness/global-setup.ts"],
    setupFiles: ["./harness/setup.ts"],
    testTimeout: 40000,
    hookTimeout: 180000,
    fileParallelism: false,
  },
})
