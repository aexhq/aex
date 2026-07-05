/**
 * Public surface of the `aex` SDK.
 *
 * `Aex` is the single SDK client. Composition primitives are `Tool`, `Skill`,
 * `McpServer`, `AgentsMd`, `File`, and `Secret`. Everything else is types,
 * errors, and event type guards re-exported from `@aexhq/contracts`.
 */

export {
  Aex,
  AgentsMdClient,
  ChildRunHandle,
  FilesClient,
  OutputsClient,
  SecretsClient,
  SessionClient,
  SessionHandle,
  SessionTurnStream,
  SkillsClient
} from "./client.js";
export type {
  AexOptions,
  BatchOptions,
  Message,
  OutputDownloadOptions,
  OutputFilePathMatch,
  OutputFilePathSelector,
  OutputFileSelector,
  OutputLinkSelector,
  PerSessionOutputSearchQuery,
  RunCollectOptions,
  RunEvents,
  RunOutputs,
  RunResult,
  SessionCreateOptions,
  SessionEnvironmentOptions,
  SessionEvents,
  SessionInput,
  SessionMessages,
  SessionOutputs,
  SessionOverrides,
  SessionRunOptions,
  SessionRunResult,
  SessionSendOptions,
  SessionTerminalRead,
  SessionTurnResult,
  SessionWebhooks,
  SettleAwait,
  StreamEventsOptions,
  SubmitResult,
  WaitForRunOptions
} from "./client.js";

// Composition primitives
export { Tool } from "./tool.js";
export { Skill } from "./skill.js";
export { AgentsMd } from "./agents-md.js";
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
  CredentialValidationError,
  ProviderError,
  RunConfigValidationError,
  RunStateError,
  apiErrorFromResponse,
  isAexApiErrorCode,
  isAuthError,
  isIdempotencyConflict,
  isInsufficientScope,
  isNotFound
} from "@aexhq/contracts";
export type { AexApiErrorCode } from "@aexhq/contracts";

// Built-in transport resilience. Every BFF request is retried on transient
// failures (429/5xx/529 + network errors) with bounded backoff + jitter,
// honoring `Retry-After`; tune or disable via the client's `retry` option.
// A persistent throttle surfaces as `AexRateLimitError` (narrow with
// `isRateLimited`), which can carry an upstream `ProviderFault`.
export { AexRateLimitError, isRateLimited, isThrottleFault, parseProviderFault } from "./retry.js";
export type { ProviderFault, RetryOptions } from "./retry.js";

// Skill-bundle / MCP wire types
export {
  MCP_SERVER_NAME_PATTERN,
  SKILL_BUNDLE_LIMITS,
  SKILL_NAME_PATTERN,
  SKILL_RESERVED_NAMES,
  SkillBundleValidationError,
  normaliseSkillBundlePath,
  validateSkillBundleEntry,
  validateSkillBundleManifest
} from "@aexhq/contracts";
export type {
  AssetRef,
  AgentsMdRef,
  FileRef,
  McpServerRef,
  SkillBundleEntry,
  SkillBundleManifest,
  SkillRecord as SkillRecordWire,
  SkillRef,
  ToolInputSchema,
  ToolRef
} from "@aexhq/contracts";

// Runtime types
export type {
  AgentsMdRecord as AgentsMdRecordWire,
  BillingCheckoutPlanKey,
  BillingCheckoutRequest,
  BillingHostedSession,
  BillingLedgerEntry,
  BillingLedgerPage,
  BillingLedgerQuery,
  BillingPortalRequest,
  BillingSummary,
  FileRecord as FileRecordWire,
  Output,
  OutputFileType,
  OutputLink,
  OutputLinkOptions,
  OutputQuery,
  OutputSearchQuery,
  OutputSearchHit,
  OutputSearchPage,
  OutputText,
  ProviderEvent,
  ReadOutputTextOptions,
  Run,
  Session,
  SessionEvent,
  SessionListPage,
  SessionListQuery,
  SessionRetentionPolicy,
  SessionStatus,
  SessionSummary,
  SessionTurn,
  RunRecordArchiveFileV1,
  RunRecordArchiveFileRoleV1,
  RunRecordArchiveNamespaceV1,
  RunRecordCostV1,
  RunRecordDownloadErrorV1,
  RunRecordFileStatusV1,
  RunRecordManifestV1,
  RunRecordMetadataV1,
  RunRecordNamespaceV1,
  RunRecordSubmissionSnapshotV1,
  RunRecordV1,
  RunWebhookDelivery,
  RunWebhookDeliveryStatus,
  RuntimeManifest,
  SecretRecord,
  UsageSummary,
  WebhookSigningSecret,
  WhoAmI
} from "@aexhq/contracts";

// Platform submission types exposed so callers can build typed environment and
// MCP secret values without depending on `@aexhq/contracts` directly.
export type {
  PlatformInlineSecrets as InlineSecrets,
  PlatformMcpServerSecret as McpServerSecret,
  PlatformEnvironment as RunEnvironment,
  PlatformRunSubmissionRequest,
  RunLimits,
  RunWebhookSpec,
} from "@aexhq/contracts";

// Runtime sizing — the closed set of valid managed runtime presets.
// Prefer the `Sizes` symbol const (e.g. `Sizes.SHARED_2X_8GB`)
// so an invalid token is a compile error, not a runtime 400.
export {
  CUSTODY_MANIFEST_SCHEMA_VERSION,
  RUN_RECORD_MANIFEST_SCHEMA_VERSION,
  RUN_RECORD_SCHEMA_VERSION,
  DEFAULT_RUNTIME_SIZE,
  RUNTIME_SIZE_PRESETS,
  RUNTIME_SIZES
} from "@aexhq/contracts";
export { RuntimeSizes as Sizes } from "@aexhq/contracts";
export type { RuntimeResources, RuntimeSize } from "@aexhq/contracts";

// Builtin tools — the closed + default builtin tool sets. Toggle the standard
// set with `includeBuiltinTools` on session create; cherry-pick individual tools by
// listing their names in `tools`. Prefer the `BuiltinTools` const (e.g.
// `BuiltinTools.web_search`) so a typo is a compile error, not a runtime 400.
export { BUILTIN_TOOL_NAMES, BuiltinTools, DEFAULT_BUILTIN_TOOLS, resolveBuiltinToolNames } from "@aexhq/contracts";
export type { BuiltinToolName } from "@aexhq/contracts";

// Provider/model surface. Provider choice decides the upstream model route;
// execution uses the managed path.
export {
  DEFAULT_RUN_PROVIDER,
  RUN_MODELS,
  RUN_MODELS_BY_PROVIDER,
  MODEL_PROVIDER_IDS,
  Models,
  providerForModel,
  providersForModel,
  resolveModelProvider,
  resolveProviderModelId,
  isRunModel,
  parseRunModel,
  Providers,
  RUN_PROVIDERS,
  suggest
} from "@aexhq/contracts";
export type {
  RunModel,
  RunProvider
} from "@aexhq/contracts";

// Unified settled-result / batch / typed-decode / lineage surface (WS3/WS8/WS10).
export { usageFromProviderUsage } from "@aexhq/contracts";
export type {
  BatchItemResult,
  BatchResult,
  ChildRunRef,
  ResolvableRunRef,
  RunOutcome,
  RunRefusalReason,
  SettledResult
} from "@aexhq/contracts";

// Status vocabulary (WS1): the terminal-outcome half + guard, bound to the run
// outcome SSoT. The bare session `error` is retired — a failed turn is `failed`.
export { SESSION_STATUSES, SESSION_TERMINAL_OUTCOMES, isTerminalSessionStatus } from "@aexhq/contracts";
export type { SessionTerminalOutcome } from "@aexhq/contracts";

// Self-describing API-key codec + plane routing (WS11): the constructor parses
// the key to derive the plane baseUrl and fail fast on a plane mismatch.
export { PLANE_BASE_URLS, parseApiKey } from "@aexhq/contracts";
export type { ApiKeyPlane, ParsedApiKey } from "@aexhq/contracts";

// Structured-output (schema-decode), HITL approval-gate, and the streaming
// capability model (WS9/WS10).
export {
  RESPONSE_FORMAT_KINDS,
  STREAMABLE_SHAPES,
  isStreamableProvider,
  parseApprovalGate,
  parseResponseFormat
} from "@aexhq/contracts";
export type { ApprovalGate, ResponseFormat, ResponseFormatKind, StreamableShape } from "@aexhq/contracts";

// Event methods. Every event the SDK yields — the turn stream (`session.send()`),
// `session.events().list()`, `session.events().streamEnvelopes()`, and
// `RunResult.events` — is an `AexEventView`: the coordinator envelope enriched
// with one type-guard METHOD per standardized event type, so a consumer branches
// with `event.isTextMessage()` / `event.isToolCallStart()` / `event.isRunError()`
// / … instead of a free-function guard or a raw `event.type === "…"` compare.
// `isTextMessage()` / `isToolCallStart()` / `isToolCallResult()` additionally
// NARROW `event.data` to that type's fields (e.g. `event.data.text` is `string`).
export { AEX_RUN_SETTLED_NAME } from "@aexhq/contracts";
export type {
  AexEventView,
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
