/**
 * The minimal C4 entrypoint copied into the SDK's private inlined contracts.
 *
 * Keep the broader `@aexhq/contracts/testing` kit out of customer packages;
 * live user tests only need the response schemas and observer installer.
 */
export {
  installWireConformance,
  formatWireConformanceReport,
  mergeWireConformanceReports,
  pathMatches,
  wireOrigin
} from "./wire-conformance.js";
export type {
  ResponseSchemaBinding,
  WireConformanceOptions,
  WireConformanceReport,
  WireConformanceViolation
} from "./wire-conformance.js";
export {
  DATA_PLANE_RESPONSE_SCHEMAS,
  ROUTES_WITHOUT_RESPONSE_SCHEMA,
  ROUTES_OFF_THE_SDK_SEAM,
  formatResponseSchemaCoverage
} from "./response-bindings.js";
export type { UnschemadRoute } from "./response-bindings.js";
