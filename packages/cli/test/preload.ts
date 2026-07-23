/**
 * bun:test preload — vitest `restoreMocks: true` parity: restore every
 * spy/mock created during a test once it finishes.
 */
import { afterEach, mock } from "bun:test";

afterEach(() => {
  mock.restore();
});
