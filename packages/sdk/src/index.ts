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
  FilesClient,
  SecretsClient,
  SessionClient,
  SessionHandle,
  SessionTurnStream,
  SkillsClient
} from "./client.js";
export type {
  AexOptions,
  Message,
  OutputDownloadOptions,
  OutputFilePathMatch,
  OutputFilePathSelector,
  OutputFileSelector,
  OutputLinkSelector,
  RunCollectOptions,
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
  SessionTurnResult,
  SessionWebhooks,
  StreamEventsOptions,
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

// Errors
export {
  AexApiError,
  AexError,
  AexNetworkError,
  CleanupError,
  CredentialValidationError,
  ProviderError,
  RunConfigValidationError,
  RunStateError
} from "@aexhq/contracts";

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
  RunEvent,
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
  resolveProviderModelId,
  isRunModel,
  parseRunModel,
  Providers,
  RUN_PROVIDERS
} from "@aexhq/contracts";
export type {
  RunModel,
  RunProvider
} from "@aexhq/contracts";

// Event guards. The lifecycle/channel guards (isRunStarted/isRunError/isCustom/
// isLog/…) operate on the coordinator `AexEvent` envelope; isTextMessage /
// isToolCallStart / isToolCallResult / isRunFinished narrow the loose `RunEvent`
// snapshot shape `listEvents` / `RunResult.events` return, typing `.data`.
export {
  AEX_RUN_SETTLED_NAME,
  isCustom,
  isEventChannel,
  isFromSource,
  isLog,
  isRunError,
  isRunFinished,
  isRunSettled,
  isRunStarted,
  isRunTerminal,
  isTextMessage,
  isToolCallResult,
  isToolCallStart
} from "@aexhq/contracts";
export type {
  RunFinishedRunEvent,
  TextMessageRunEvent,
  ToolCallResultRunEvent,
  ToolCallStartRunEvent
} from "@aexhq/contracts";

// Secret utilities
export { SecretString, redactSecrets } from "@aexhq/contracts";

// Webhook verification — customers verify inbound run webhooks (Standard
// Webhooks scheme) with `verifyAexWebhook(...)`, no extra dependency needed.
export { verifyAexWebhook } from "@aexhq/contracts";
export type { VerifyAexWebhookInput } from "@aexhq/contracts";
