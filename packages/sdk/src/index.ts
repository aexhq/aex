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
  PersistedSessionFilesClient,
  RegisteredResourceClient,
  RunFailedError,
  RunHandle,
  SessionApprovalsClient,
  SessionCredentialsClient,
  SessionFilesClient,
  SessionHandle,
  LiveSessionFilesClient,
  SessionMessagesClient,
  SessionWorkspaceClient,
  SessionsClient,
  WorkspaceClient,
  WorkspaceSecretsClient,
  WorkspaceUploadsClient
} from "./client-v1.js";

export type {
  AexOptions,
  ApprovalListQuery,
  CredentialRebindRequest,
  IdempotencyOptions,
  MessagePart,
  MessageSendAccepted,
  OperationAdmissionOptions,
  OperationListQuery,
  OperationResult,
  RevisionMutationOptions,
  RevisionOperationAdmissionOptions,
  RevisionOptions,
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
  Approval,
  ApprovalResponseRequest,
  BlobInput,
  DownloadGrant,
  FileDownloadRequest,
  FileEntry,
  Id,
  LiveDownloadGrant,
  LiveFileDownloadRequest,
  LiveFileEntry,
  LiveFileListRequest,
  LiveFilePage,
  LiveFileStatRequest,
  MessageSendRequest,
  MessageV1 as Message,
  Operation,
  OperationKind,
  OperationStatusV1 as OperationStatus,
  Page,
  PersistedFileListRequest,
  PersistedFileStatRequest,
  RegisteredFileValue,
  RegisteredInstructionValue,
  RegisteredMcpServerValue,
  RegisteredResource,
  RegisteredResourceKind,
  RegisteredResourceSummary,
  RegisteredSkillValue,
  RegisteredToolValue,
  Region,
  RunStatusV1 as RunStatus,
  RunV1 as Run,
  SessionCreateRequestV1 as SessionCreateRequest,
  SessionStatusV1 as SessionStatus,
  SessionV1 as Session,
  SecretMetadataV1 as SecretMetadata,
  SecretRevocation,
  Upload,
  UploadCompleteRequest,
  UploadCreateRequest,
  UploadPartGrant,
  UploadPartsRequest,
  UploadPartsResponse,
  WorkspaceAccess,
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
