/**
 * bun:test preload — loads `.env.local` (live target vars, provider keys)
 * before any test file is collected, replacing the `loadLocalEnv()` call the
 * retired vitest lane configs made at config-eval time. Runs once per test
 * process (`--isolate` gives one process per file).
 *
 * No mock-restore hook here on purpose: this package spawns child processes
 * instead of mocking (zero spies/mocks in the suites).
 */
import { loadLocalEnv } from "./env-local";

loadLocalEnv();
