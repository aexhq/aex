/**
 * Public surface of the `aex` SDK.
 *
 * `Aex` is the single SDK client. Composition primitives are `Tool`, `Skill`,
 * `McpServer`, `Instructions`, `File`, and `Secret`. Everything else is types,
 * errors, and event type guards re-exported from `@aexhq/contracts`.
 */

export { Aex } from "./client.js";
export type {
  ChildSessionHandle,
  SessionClient,
  SessionHandle,
  SessionRunStream,
  WorkspaceClient,
  WorkspaceFilesClient,
  WorkspaceInstructionsClient,
  WorkspaceSkillsClient,
  WorkspaceToolsClient,
  OrgsClient,
  WorkspacesClient,
  KeysClient
} from "./client.js";
// Control-plane one-time reveal results — the minted key is a redacted
// `SecretString` (call `.unwrap()` to read it).
export type { NewWorkspaceResult, NewApiKeyResult } from "./client.js";
export type {
  AexOptions,
  ChildSessionEvents,
  IterateEventsOptions,
  Message,
  DownloadOptions,
  SessionFilePathMatch,
  SessionFilePathSelector,
  SessionFileSelector,
  SessionFileLinkSelector,
  StartSessionOptions,
  SessionFiles,
  SessionResult,
  SessionCreateOptions,
  SessionEnvironmentOptions,
  SessionEvents,
  SessionInput,
  SessionMessages,
  SessionOtel,
  SessionOverrides,
  SessionStartOptions,
  SessionSendOptions,
  SessionRunResult,
  SessionWebhooks,
  StreamEventsOptions
} from "./client.js";

// Composition primitives
export { Tool } from "./tool.js";
export { Skill } from "./skill.js";
export { Instructions } from "./instructions.js";
export { File } from "./file.js";
export { McpServer } from "./mcp-server.js";
export { Secret } from "./secret.js";
export type { SecretEnvSubmissionEntry } from "./secret.js";
export { bundleSkillFiles, hashSkillBundle } from "./bundle.js";
export type { BundledSkill, BundledTool, BundleMeta, SkillFiles, ToolBundleManifest } from "./bundle.js";

// Errors. One wire→exception factory (`apiErrorFromResponse`) + a stable
// `apiCode` on every `AexApiError`; typed subclasses (`AexAuthError` /
// `AexIdempotencyConflictError` / `AexNotFoundError`) each narrow with a guard.
export {
  AEX_API_ERROR_CODES,
  AEX_API_ERROR_MESSAGES,
  AEX_API_ERROR_REMEDIES,
  AexApiError,
  AexAuthError,
  AexError,
  AexIdempotencyConflictError,
  AexNetworkError,
  AexNotFoundError,
  CleanupError,
  ContentDeletedError,
  CredentialValidationError,
  ProviderError,
  SessionConfigValidationError,
  SessionStateError,
  apiErrorFromResponse,
  isAexApiErrorCode,
  isAuthError,
  isContentDeleted,
  isIdempotencyConflict,
  isInsufficientScope,
  isNotFound
} from "@aexhq/contracts";
export type { AexApiErrorCode } from "@aexhq/contracts";

// Built-in transport resilience. Safe reads and mutations carrying a stable
// Idempotency-Key are retried on transient failures
// (429/5xx/529 + network errors) with bounded backoff + jitter, honoring
// `Retry-After`; tune or disable via the client's `retry` option.
// A persistent throttle surfaces as `AexRateLimitError` (narrow with
// `isRateLimited`), which can carry an upstream `ProviderFault`.
export { AexRateLimitError, isRateLimited, isThrottleFault, parseProviderFault } from "./retry.js";
export type { ProviderFault, RetryOptions } from "./retry.js";

// Skill-bundle / MCP wire types
export {
  ASSET_ARCHIVE_LIMITS,
  MCP_SERVER_NAME_PATTERN,
  SKILL_BUNDLE_LIMITS,
  SKILL_NAME_PATTERN,
  SKILL_RESERVED_NAMES,
  SkillBundleValidationError,
  normaliseSkillBundlePath,
  parseSkillBundleEntry,
  parseSkillBundleManifest,
  validateSkillBundleEntry,
  validateSkillBundleManifest
} from "@aexhq/contracts";
export type {
  McpServerRef,
  SkillBundleEntry,
  SkillBundleManifest,
  ToolInputSchema,
} from "@aexhq/contracts";

export type {
  AssetIdentity,
  SubmissionAssets,
  WorkspaceFileRecord,
  WorkspaceFileRef,
  WorkspaceInstructionRecord,
  WorkspaceInstructionRef,
  WorkspaceSkillRecord,
  WorkspaceSkillRef,
  WorkspaceToolRecord,
  WorkspaceToolRef
} from "@aexhq/contracts";

// Runtime types
export type {
  BillingAdmissionState,
  BillingAllowance,
  BillingAutoTopup,
  BillingAutoTopupRequest,
  BillingAutoTopupUpdate,
  BillingBlock,
  BillingHostedSession,
  BillingLedgerEntry,
  BillingLedgerPage,
  BillingLedgerQuery,
  BillingPaymentMethod,
  BillingPortalRequest,
  BillingSummary,
  BillingTopupCheckoutRequest,
  SessionFile,
  SessionFilesSnapshot,
  SessionCheckpointRevision,
  SessionFileType,
  SessionFileLink,
  SessionFileLinkOptions,
  SessionFileQuery,
  SessionFileText,
  ReadSessionFileTextOptions,
  Session,
  SessionListPage,
  SessionListQuery,
  SessionRetentionPolicy,
  SessionRuntime,
  SessionStatus,
  SessionSummary,
  SessionRun,
  SessionRecordArchiveFileV1,
  SessionRecordArchiveFileRoleV1,
  SessionRecordArchiveNamespaceV1,
  SessionRecordCostV1,
  SessionRecordDownloadErrorV1,
  SessionRecordFileStatusV1,
  SessionRecordManifestV1,
  SessionRecordMetadataV1,
  SessionRecordNamespaceV1,
  SessionRecordSubmissionSnapshotV1,
  SessionRecordV1,
  SessionRunErrorWebhookPayload,
  SessionRunFinishedWebhookPayload,
  SessionRunWebhookData,
  SessionRunWebhookEventType,
  SessionRunWebhookPayload,
  SessionWebhookDelivery,
  SessionWebhookDeliveryStatus,
  RuntimeManifest,
  SecretRecord,
  UsageSummary,
  WebhookSigningSecret,
  WhoAmI,
  OrgRecord,
  CreateOrgRequest,
  WorkspaceRecord,
  CreateWorkspaceRequest,
  NewWorkspace,
  ApiKeyRecord,
  CreateApiKeyRequest,
  NewApiKey,
  OrgMemberRecord,
  CreateOrgInviteRequest,
  OrgInvite
} from "@aexhq/contracts";

// Platform submission types exposed so callers can build typed environment and
// MCP secret values without depending on `@aexhq/contracts` directly.
export type {
  PlatformInlineSecrets as InlineSecrets,
  PlatformMcpServerSecret as McpServerSecret,
  PlatformEnvironment as SessionEnvironment,
  SessionLimits,
  SessionWebhookSpec,
} from "@aexhq/contracts";

// Runtime sizing — the closed set of valid managed runtime presets.
// Prefer the `Sizes` symbol const (e.g. `Sizes.CPU_2_8GB`)
// so an invalid token is a compile error, not a runtime 400.
export {
  SESSION_RECORD_MANIFEST_SCHEMA_VERSION,
  SESSION_RECORD_SCHEMA_VERSION,
  DEFAULT_RUNTIME_SIZE,
  RUNTIME_SIZE_PRESETS,
  RUNTIME_SIZES
} from "@aexhq/contracts";
export { RuntimeSizes as Sizes } from "@aexhq/contracts";
export type { RuntimeResources, RuntimeSize } from "@aexhq/contracts";

// Execution runtime — which backend runs the session (distinct from size).
// Prefer the `RuntimeKinds` symbol const (e.g. `RuntimeKinds.SPOT_CONTAINER`).
export { DEFAULT_RUNTIME_KIND, RUNTIME_KINDS, RuntimeKinds } from "@aexhq/contracts";
export type { RuntimeKind } from "@aexhq/contracts";

// Builtin tools — the closed + default builtin tool sets. Select `"default"`,
// `"none"`, or individual names with `builtinTools`. Prefer the `BuiltinTools` const (e.g.
// `BuiltinTools.web_search`) so a typo is a compile error, not a runtime 400.
export { BUILTIN_TOOL_NAMES, BuiltinTools, DEFAULT_BUILTIN_TOOLS, resolveBuiltinToolNames } from "@aexhq/contracts";
export type { BuiltinToolName } from "@aexhq/contracts";

// Model surface. Public model ids are plain Vercel AI Gateway `creator/model`
// slug strings validated structurally by `parseModelSlug`; the managed gateway
// routes them (no provider selection, no API key). `ProviderName` survives only
// as a serving-provider telemetry string alias.
export {
  MODEL_SLUG_PATTERN,
  isModelSlug,
  parseModelSlug,
  suggest
} from "@aexhq/contracts";
export type {
  ModelName,
  ProviderName
} from "@aexhq/contracts";

// Unified run result, typed decode, and lineage surface.
export { usageFromProviderUsage } from "@aexhq/contracts";
export type {
  ChildSessionRef,
  TurnOutcome,
  TurnRefusalReason,
  TurnResult
} from "@aexhq/contracts";

// Session lifecycle vocabulary and guards. Run verdicts are reported separately
// by `lastRun.outcome` and terminal RUN events.
export { SESSION_STATUSES, SESSION_TERMINAL_OUTCOMES, isTerminalSessionStatus } from "@aexhq/contracts";
export type { SessionTerminalOutcome } from "@aexhq/contracts";

// Self-describing API-key codec + plane routing (WS11): the constructor parses
// the key to derive the plane baseUrl and fail fast on a plane mismatch.
export {
  CONTRACT_PARSE_ERROR,
  PLANE_BASE_URLS,
  isContractParseError,
  parseApiKey,
  tryParseApiKey
} from "@aexhq/contracts";
export type { ApiKeyPlane, ContractParseError, ParsedApiKey } from "@aexhq/contracts";

// Structured-output (schema-decode) and HITL approval-gate (WS10). Streaming is
// allowed for all models through the managed gateway — there is no capability gate.
export {
  RESPONSE_FORMAT_KINDS,
  parseApprovalGate,
  parseResponseFormat
} from "@aexhq/contracts";
export type { ApprovalGate, ResponseFormat, ResponseFormatKind } from "@aexhq/contracts";

// Event methods. Durable list/archive/result events are `AexEventView` values
// with a replay `sequence`. Live run/coordinator streams yield
// `AexStreamEventView`, which also admits provisional `replayable:false` events
// with a per-run `liveSequence` and no durable sequence. Both views are enriched
// with one type-guard METHOD per standardized event type, so a consumer branches
// with `event.isTextMessage()` / `event.isToolCallStart()` / `event.isRunError()`
// / … instead of a free-function guard or a raw `event.type === "…"` compare.
// `isTextMessage()` / `isToolCallStart()` / `isToolCallResult()` additionally
// NARROW `event.data` to that type's fields (e.g. `event.data.text` is `string`).
export type {
  AexEvent,
  AexEventBase,
  AexEventView,
  AexLiveEvent,
  AexLiveEventView,
  OtlpAnyValue,
  OtlpExportLogsServiceRequest,
  OtlpExportRequest,
  OtlpExportTraceServiceRequest,
  OtlpInstrumentationScope,
  OtlpKeyValue,
  OtlpLogRecord,
  OtlpResource,
  OtlpResourceLogs,
  OtlpResourceSpans,
  OtlpScopeLogs,
  OtlpScopeSpans,
  OtlpSignal,
  OtlpTraceSpan,
  AexStreamEvent,
  AexStreamEventView,
  TextMessageEventView,
  ToolCallResultEventView,
  ToolCallStartEventView
} from "@aexhq/contracts";

// Secret utilities
export { SecretString, redactSecrets } from "@aexhq/contracts";

// Webhook verification — customers verify inbound run webhooks (Standard
// Webhooks scheme) with `verifyAexWebhook(...)`, no extra dependency needed.
export { verifyAexWebhook } from "@aexhq/contracts";
export type { VerifyAexWebhookInput } from "@aexhq/contracts";
