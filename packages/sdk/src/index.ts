/**
 * Public surface of the `aex` SDK.
 *
 * ONE class (`AgentExecutor`) talks to the dashboard BFF. The CLI exposes
 * the SAME operations via subcommands. Composition primitives are
 * `Skill`, `McpServer`, and `ProxyEndpoint` — there is no saved-definition
 * wrapper. Everything else is types, errors, and event type guards re-exported
 * from `@aexhq/contracts`.
 */

export { AgentsMdClient, AgentExecutor, FilesClient, SecretsClient, SkillsClient } from "./client.js";
export type {
  AgentExecutorOptions,
  OutputDownloadOptions,
  OutputFilePathMatch,
  OutputFilePathSelector,
  OutputFileSelector,
  OutputLinkSelector,
  RunCollectOptions,
  RunResult,
  StreamEventsOptions,
  SubmitOptions,
  WaitForRunOptions
} from "./client.js";

// Composition primitives
export { Skill } from "./skill.js";
export { Tool } from "./tool.js";
export { AgentsMd } from "./agents-md.js";
export { File } from "./file.js";
export { McpServer } from "./mcp-server.js";
export { ProxyEndpoint } from "./proxy-endpoint.js";
export { Secret } from "./secret.js";
export type { SecretEnvSubmissionEntry } from "./secret.js";
export type {
  BearerProxyEndpointOptions,
  BasicProxyEndpointOptions,
  HeaderProxyEndpointOptions,
  ProxyEndpointCommonOptions,
  QueryProxyEndpointOptions
} from "./proxy-endpoint.js";
export { bundleSkillFiles, hashSkillBundle } from "./bundle.js";
export type { BundledSkill, BundledTool, SkillFiles, ToolBundleManifest } from "./bundle.js";

// Data-source chat tools — turn the read surface (listRuns / listOutputs /
// readOutputText) into vendor-neutral LLM tool definitions + an executor, so a
// chat over workspace/run data is a few lines on top of the public SDK.
export { createDataTools, createCorpusTools, DataToolError, DATA_TOOLS_INSTRUCTIONS } from "./data-tools.js";
export type { ChatCorpus, CreateDataToolsOptions, DataChatTool, DataChatToolSchema, DataTools } from "./data-tools.js";

// Errors
export {
  AexApiError,
  AexError,
  CleanupError,
  CredentialValidationError,
  ProviderError,
  RunConfigValidationError,
  RunStateError
} from "@aexhq/contracts";

// Skill / MCP wire types
export {
  MCP_SERVER_NAME_PATTERN,
  SKILL_BUNDLE_LIMITS,
  SkillBundleValidationError,
  buildPlatformAllowedHosts,
  normaliseSkillBundlePath,
  validateSkillBundleEntry,
  validateSkillBundleManifest,
  validateProxyAuth
} from "@aexhq/contracts";
export type {
  AssetRef,
  AgentsMdRef,
  FileRef,
  McpServerRef,
  SkillBundleEntry,
  SkillBundleManifest,
  SkillRef,
  ToolInputSchema,
  ToolRef
} from "@aexhq/contracts";

// Runtime types
export type {
  AgentsMdRecord as AgentsMdRecordWire,
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
  RunListPage,
  RunListQuery,
  RunSummary,
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
  SecretReveal,
  SignedOutputLink,
  Skill as SkillRecord,
  UsageSummary,
  WhoAmI
} from "@aexhq/contracts";

// Platform submission types — exposed so callers can build typed `secrets`
// arrays without depending on `@aexhq/contracts` directly. The raw
// `PlatformProxyEndpoint` wire shape is intentionally re-exported under
// its full name; the user-facing constructor for proxy endpoints is the
// `ProxyEndpoint` class (above), which prevents the wire-format
// mistakes agents hit when authoring the wire shape by hand.
export type {
  PlatformInlineSecrets as InlineSecrets,
  PlatformMcpServerSecret as McpServerSecret,
  PlatformProxyEndpoint,
  PlatformProxyEndpointAuth,
  PlatformProxyAuthValue as ProxyAuthValue,
  PlatformEnvironment as RunEnvironment,
  PlatformRunSubmissionRequest,
  RunLimits,
  RunWebhookSpec,
  ProxyAuthShape,
  ProxyMethod,
  ProxyRetryPolicy,
  ProxyResponseMode
} from "@aexhq/contracts";

// Runtime sizing — the closed set of valid managed runtime presets.
// Prefer the `RuntimeSizes` symbol const (e.g. `RuntimeSizes.SHARED_2X_8GB`)
// so an invalid token is a compile error, not a runtime 400.
export {
  CUSTODY_MANIFEST_SCHEMA_VERSION,
  RUN_RECORD_MANIFEST_SCHEMA_VERSION,
  RUN_RECORD_SCHEMA_VERSION,
  DEFAULT_RUNTIME_SIZE,
  RUNTIME_SIZE_PRESETS,
  RUNTIME_SIZES,
  RuntimeSizes
} from "@aexhq/contracts";
export type { RuntimeResources, RuntimeSize } from "@aexhq/contracts";

// Builtin tools — the closed + default builtin tool sets. Toggle the standard
// set with `includeBuiltinTools` on submit; cherry-pick individual tools by
// listing their names in `tools`. Prefer the `BuiltinTools` const (e.g.
// `BuiltinTools.notebook_edit`) so a typo is a compile error, not a runtime 400.
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
  RunModels,
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

// Typed `listEvents` decoders — correlate TOOL_CALL_START/RESULT into tool-call
// traces, tolerate historical/internal `aex.usage` records when present, and
// decode assistant text, so consumers don't hand-roll the `data.id` correlation.
export {
  decodeAssistantText,
  decodeToolCalls,
  summarizeRunTrace,
  summarizeRunUsage,
  textOf
} from "@aexhq/contracts";
export type {
  AssistantTextEntry,
  RunTrace,
  ToolCallResult,
  ToolCallTrace
} from "@aexhq/contracts";

// Secret utilities
export { SecretString, redactSecrets } from "@aexhq/contracts";

// Webhook verification — customers verify inbound run webhooks (Standard
// Webhooks scheme) with `verifyAexWebhook(...)`, no extra dependency needed.
export { verifyAexWebhook } from "@aexhq/contracts";
export type { VerifyAexWebhookInput } from "@aexhq/contracts";
