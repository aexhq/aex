/**
 * `@aexhq/contracts/testing` — runner-agnostic test kit + canonical fakes.
 *
 * Nothing here imports a test runner: helpers stay runner-agnostic (they
 * ran under vitest before the bun:test migration unchanged); callers wire
 * their own lifecycle hooks (e.g.
 * `afterEach(restoreEnv)`).
 */
export { stubEnv, restoreEnv } from "./testing/stub-env.js";
export { createFakeTimers, withFakeClock, type FakeClock, type FakeTimers } from "./testing/fake-clock.js";
export { waitForCondition, type WaitForConditionOptions } from "./testing/wait-for-condition.js";
export {
  caps,
  testPlatformCapabilities,
  type TestPlatformCapabilities
} from "./testing/test-platform.js";
export { FakeWebSocket } from "./testing/fake-web-socket.js";
export {
  FakeCustodyManifestObjectStore,
  FakeSessionDeletionManifestObjectStore
} from "./testing/fake-object-stores.js";
