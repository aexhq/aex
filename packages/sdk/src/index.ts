export { Aex } from "./client/aex.js";
export type { AexOptions } from "./client/aex.js";
export {
  AccountToken,
  WorkspaceApiKey,
  parseCredential,
  regionalHost,
} from "./client/credentials.js";
export type { ParsedCredential, RegionCode } from "./client/credentials.js";
export { resolveCentralBaseUrl, resolveRegionalBaseUrl } from "./client/routing.js";
export type { RegionalRoutingOptions } from "./client/routing.js";
export {
  AexApiError,
  AexAuthError,
  AexConfigError,
  AexConflictError,
  AexError,
  AexGoneError,
  AexInternalError,
  AexNotFoundError,
  AexPreconditionError,
  AexQuotaError,
  AexStateError,
  AexStreamProtocolError,
  AexUnavailableError,
  AexValidationError,
  apiErrorFromResponse,
  isRetryable,
} from "./transport/errors.js";
export type { AexErrorCode } from "./transport/errors.js";
export { RETRY_POLICY, executeWithRetry } from "./transport/retry.js";
export { FetchTransport } from "./transport/transport.js";
export type { AexTransport, WireRequest, WireResponse } from "./transport/transport.js";
export { Page } from "./transport/pagination.js";
export { Download, MAX_SINGLE_GET_BYTES, planDownloadRanges } from "./downloads/download.js";
export type { DownloadGrant, DownloadRange } from "./downloads/download.js";
export { parseNdjsonFrames } from "./observations/stream.js";
export { ERROR_METADATA } from "./generated/errors.js";
export type { ErrorClass } from "./generated/errors.js";
export { ROUTES } from "./generated/routes.js";
export type { RouteDescriptor, RouteId } from "./generated/routes.js";
// The resource classes themselves are reached through `Aex`, not named at the
// root: their set is derived from the deferral ledger, and a root export list
// that moves whenever a route lands is a list nobody can review.
export type { ExecuteOptions, ResourceExecutor } from "./generated/resources.js";
