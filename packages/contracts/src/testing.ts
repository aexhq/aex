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

/**
 * C4 — the wire-conformance harness and the response schemas it checks against.
 *
 * Exported here because the platform's user-test suites consume the PUBLISHED
 * package. A harness those suites cannot import is a harness that never runs,
 * and an unrun gate is the exact failure this plan exists to attack.
 */
export {
  installWireConformance,
  formatWireConformanceReport,
  pathMatches,
  type ResponseSchemaBinding,
  type WireConformanceReport,
  type WireConformanceViolation
} from "./testing/wire-conformance.js";
export {
  DATA_PLANE_RESPONSE_SCHEMAS,
  ROUTES_WITHOUT_RESPONSE_SCHEMA,
  ROUTES_OFF_THE_SDK_SEAM,
  formatResponseSchemaCoverage,
  type UnschemadRoute
} from "./testing/response-bindings.js";
export { observeWireResponses, type WireResponse } from "./wire-observer.js";
