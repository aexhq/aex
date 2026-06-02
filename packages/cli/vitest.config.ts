import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "node",
    globals: true,
    restoreMocks: true,
    coverage: {
      provider: "v8",
      reporter: ["text", "json-summary"],
      exclude: ["dist/**", "coverage/**", "scripts/**", "**/*.config.*"]
    }
  }
});
