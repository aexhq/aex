/**
 * Public v1 contract surface.
 *
 * The generated bootstrap and regional OpenAPI documents are the route
 * authority. This barrel exposes only the strict v1 resources and the small
 * transport/identity vocabulary needed by public clients.
 */
export { AEX_DEFAULT_BASE_URL } from "./stable.js";
export * from "./sdk-errors.js";
export * from "./error-codes.js";
export * from "./error-factory.js";
export * from "./http.js";
export * from "./ids.js";
export * from "./api-key.js";
export * from "./api-routes.js";
export * from "./v1-resources.js";
export * from "./v1-content.js";
export * from "./v1-telemetry.js";
export * from "./v1-account-billing.js";
