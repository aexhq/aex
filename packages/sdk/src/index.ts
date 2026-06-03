/**
 * Public surface of the `antpath` SDK.
 *
 * ONE class (`AntpathClient`) talks to the dashboard BFF. The CLI exposes
 * the SAME operations via subcommands. Composition primitives are
 * `Skill`, `McpServer`, and `ProxyEndpoint` — there is no saved-definition
 * wrapper. Everything else is types, errors, and event type guards re-exported
 * from `@antpath/contracts`.
 */

export { AgentsMdClient, AntpathClient, FilesClient, SkillsClient } from "./client.js";
export type {
  AntpathClientOptions,
  OutputDownloadOptions,
  OutputFilePathMatch,
  OutputFilePathSelector,
  OutputFileSelector,
  RunDebugLog,
  RunDebugLogError,
  RunDebugLogs,
  StreamEventsOptions,
  SubmitRunOptions,
  WaitForRunOptions
} from "./client.js";

// Composition primitives
export { Skill } from "./skill.js";
export { AgentsMd } from "./agents-md.js";
export { File } from "./file.js";
export { McpServer } from "./mcp-server.js";
export { ProxyEndpoint } from "./proxy-endpoint.js";
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
  AntpathApiError,
  AntpathError,
  CleanupError,
  CredentialValidationError,
  ProviderError,
  RunStateError
} from "@antpath/contracts";

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
} from "@antpath/contracts";
export type {
  AssetRef,
  AgentsMdRef,
  FileRef,
  McpServerRef,
  SkillBundleEntry,
  SkillBundleManifest,
  SkillRef
} from "@antpath/contracts";

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
  SignedOutputLink,
  Skill as SkillRecord,
  UsageSummary,
  WhoAmI
} from "@antpath/contracts";

// Platform submission types — exposed so callers can build typed `secrets`
// arrays without depending on `@antpath/contracts` directly. The raw
// `PlatformProxyEndpoint` wire shape is intentionally re-exported under
// its full name; the user-facing constructor for proxy endpoints is the
// `ProxyEndpoint` class (above), which prevents the wire-format
// mistakes agents hit when authoring the wire shape by hand.
export type {
  PlatformAnthropicSecrets as AnthropicSecrets,
  PlatformInlineSecrets as InlineSecrets,
  PlatformMcpServerSecret as McpServerSecret,
  PlatformProxyEndpoint,
  PlatformProxyEndpointAuth,
  PlatformProxyAuthValue as ProxyAuthValue,
  PlatformEnvironment as RunEnvironment,
  PlatformRunSubmissionRequest,
  ProxyAuthShape,
  ProxyMethod,
  ProxyResponseMode
} from "@antpath/contracts";

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
} from "@antpath/contracts";
export type { RuntimeResources, RuntimeSize } from "@antpath/contracts";

// Provider + runtime dispatch surface. Agents and SDK consumers
// inspect these to know which (provider, runtime) combos are valid
// and to validate submissions offline before posting.
export {
  CREDENTIAL_MODES,
  DEFAULT_CREDENTIAL_MODE,
  collectManagedUnsupportedFeatures,
  DEFAULT_RUN_PROVIDER,
  RUN_PROVIDERS,
  RUNTIME_KINDS,
  RUNTIME_VALIDATION_CODES,
  RuntimeValidationError,
  selectRuntime
} from "@antpath/contracts";
export type {
  CredentialMode,
  RunProvider,
  RuntimeKind,
  RuntimeValidationCode
} from "@antpath/contracts";

// Normalized coordinator event guards
export {
  isCustom,
  isEventChannel,
  isFromSource,
  isLog,
  isRunError,
  isRunFinished,
  isRunStarted,
  isRunTerminal,
  isTextMessage,
  isToolCallResult,
  isToolCallStart
} from "@antpath/contracts";

// Secret utilities
export { SecretString, redactSecrets } from "@antpath/contracts";
