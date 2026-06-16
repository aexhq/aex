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
  StreamEventsOptions,
  SubmitOptions,
  SubmitRunOptions,
  WaitForRunOptions
} from "./client.js";

// Composition primitives
export { Skill } from "./skill.js";
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
export type { BundledSkill, SkillFiles } from "./bundle.js";

// Errors
export {
  AexApiError,
  AexError,
  CleanupError,
  CredentialValidationError,
  ProviderError,
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
  SkillRef
} from "@aexhq/contracts";

// Runtime types
export type {
  AgentsMdRecord as AgentsMdRecordWire,
  FileRecord as FileRecordWire,
  Output,
  ProviderEvent,
  Run,
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
  RuntimeManifest,
  RuntimeProvider,
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
  ProxyAuthShape,
  ProxyMethod,
  ProxyRetryPolicy,
  ProxyResponseMode
} from "@aexhq/contracts";

// Runtime sizing — the closed set of valid managed runtime presets.
// Prefer the `RuntimeSizes` symbol const (e.g. `RuntimeSizes.SHARED_2X_2GB`)
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

// Managed-runtime builtin extensions — the closed builtin set.
// Prefer the `Builtins` symbol const (e.g.
// `Builtins.COMPUTER_CONTROLLER`) so an invalid token is a compile
// error, not a runtime 400.
export { BUILTINS, Builtins } from "@aexhq/contracts";
export type { Builtin } from "@aexhq/contracts";

// Provider + runtime dispatch surface. Agents and SDK consumers
// inspect these to know which (provider, runtime) combos are valid
// and to validate submissions offline before posting.
export {
  CREDENTIAL_MODES,
  DEFAULT_CREDENTIAL_MODE,
  collectManagedUnsupportedFeatures,
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
  RUN_PROVIDERS,
  RUNTIME_KINDS,
  RUNTIME_VALIDATION_CODES,
  RuntimeValidationError,
  selectRuntime
} from "@aexhq/contracts";
export type {
  CredentialMode,
  RunModel,
  RunProvider,
  RuntimeKind,
  RuntimeValidationCode
} from "@aexhq/contracts";

// Normalized coordinator event guards
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

// Secret utilities
export { SecretString, redactSecrets } from "@aexhq/contracts";
