/**
 * bun:test preload — restores every spy/mock after each test, replacing the
 * `restoreMocks: true` vitest config this package used before the runner flip.
 */
import { afterEach, mock } from "bun:test";

afterEach(() => {
  mock.restore();
});
