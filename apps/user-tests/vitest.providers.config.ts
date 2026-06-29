import { defineConfig } from "vitest/config";
import { loadLocalEnv } from "./test/env-local";

loadLocalEnv();

/**
 * Dedicated config for the on-demand per-provider correctness suite
 * (`bun run test:user:providers`). Each file under test/live/providers/ is a
 * single minimal round-trip that proves one extra provider's
 * adapter/routing/registry wiring reaches its real upstream — feature depth is
 * already covered on the two wire shapes (openai-chat via DeepSeek,
 * anthropic-messages via Anthropic) by the default sweep, so these providers
 * (doubao, and future openai/gemini/mistral/openrouter) need only a
 * connectivity check.
 *
 * Kept separate from vitest.config.ts (which EXCLUDES test/live/providers/**)
 * so this runs ONLY when invoked explicitly — never on every push — which is
 * what keeps the extra-provider matrix from piling live-run spend. Each
 * provider test fails fast when its key is absent, so invoking this script is a
 * hard gate for the advertised provider evidence in this suite.
 */
export default defineConfig({
  test: {
    environment: "node",
    globals: true,
    testTimeout: 180_000,
    hookTimeout: 180_000,
    include: ["test/live/providers/**/*.test.ts"],
    exclude: ["**/node_modules/**"],
    // Each cell spawns its own child process against the live hosted API and
    // costs real money; serialize so providers don't pile spend on at once.
    fileParallelism: false,
    sequence: {
      concurrent: false
    }
  }
});
