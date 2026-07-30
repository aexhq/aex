/**
 * Public v1 SDK surface.
 *
 * Execution is explicitly session-oriented. Long-running mutations return
 * durable operation handles; run and operation waits are client-side GET
 * polling conveniences, not separate REST resources.
 */
export {
  Aex,
  OperationFailedError,
  OperationHandle,
  OperationsClient,
  RunFailedError,
  RunHandle,
  SessionCredentialsClient,
  SessionHandle,
  SessionMessagesClient,
  SessionWorkspaceClient,
  SessionsClient
} from "./client-v1.js";

export type {
  AexOptions,
  CredentialRebindRequest,
  IdempotencyOptions,
  MessagePart,
  MessageSendAccepted,
  OperationAdmissionOptions,
  OperationListQuery,
  OperationResult,
  RevisionOperationAdmissionOptions,
  SessionDeleteRequest,
  SessionForkRequest,
  SessionListQuery,
  SessionPersistRequest,
  WaitOptions,
  WorkspaceDiscardRequest
} from "./client-v1.js";

export type {
  ApiError,
  ApiErrorBody,
  Id,
  MessageSendRequest,
  MessageV1 as Message,
  Operation,
  OperationKind,
  OperationStatusV1 as OperationStatus,
  Page,
  Region,
  RunStatusV1 as RunStatus,
  RunV1 as Run,
  SessionCreateRequestV1 as SessionCreateRequest,
  SessionStatusV1 as SessionStatus,
  SessionV1 as Session,
  WorkspaceContinuity
} from "@aexhq/contracts";

export {
  AexApiError,
  AexAuthError,
  AexError,
  AexIdempotencyConflictError,
  AexNetworkError,
  AexNotFoundError,
  CredentialValidationError,
  apiErrorFromResponse,
  isAuthError,
  isIdempotencyConflict,
  isNotFound
} from "@aexhq/contracts";

// Registered-resource authoring primitives remain usable while their v1
// registry clients land in the next public SDK wave.
export { File } from "./file.js";
export { Instructions } from "./instructions.js";
export { Secret } from "./secret.js";
export { Skill } from "./skill.js";
export { Tool } from "./tool.js";
