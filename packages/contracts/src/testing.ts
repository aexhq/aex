/**
 * `@aexhq/contracts/testing` — runner-agnostic test kit + canonical fakes.
 *
 * Nothing here imports a test runner: helpers work under vitest today and
 * bun:test after migration; callers wire their own lifecycle hooks (e.g.
 * `afterEach(restoreEnv)`).
 */
export { stubEnv, restoreEnv } from "./testing/stub-env.js";
export { withFakeClock, type FakeClock } from "./testing/fake-clock.js";
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
