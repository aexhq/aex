import {
  SKILL_NAME_PATTERN,
  SKILL_RESERVED_NAMES,
  TOOL_NAME_PATTERN,
  assertValidMountPath,
  normaliseSkillBundlePath,
  parseMcpServerRef
} from "./session-config.js";
import type { McpServerRef, ToolInputSchema } from "./session-config.js";
import { parseSessionTimeout, parseRuntimeSize, type RuntimeSize } from "./runtime-sizes.js";
import { parseRuntimeKind, type RuntimeKind } from "./runtime-kind.js";
import {
  parseModelSlug,
  type ModelName
} from "./models.js";
import {
  parseRuntimeSecurityProfile,
  type RuntimeSecurityProfileName
} from "./runtime-security-profile.js";
import type {
  SubmissionAssets,
  WorkspaceFileRef,
  WorkspaceInstructionRef,
  WorkspaceSkillRef,
  WorkspaceToolRef
} from "./workspace-resources.js";
import {
  assertPinnedWorkspaceResource,
  assertWorkspaceFileResourceName,
  assertWorkspaceInstructionResourceName
} from "./workspace-resources.js";
import { withContractParseError } from "./contract-parse-error.js";
import { parseWire } from "./schemas/wire.js";
import { SessionSubmissionRequestSchema } from "./schemas/submission-request.js";
import {
  ApprovalGateSchema,
  FileCaptureSchema,
  PlatformInjectionSchema,
  ResponseFormatJsonSchemaSchema,
  ResponseFormatTextSchema,
  SubmissionSchema
} from "./schemas/submission-body.js";
import {
  SubmissionAssetsSchema,
  gateWorkspaceFileRef,
  gateWorkspaceInstructionRef,
  gateWorkspaceSkillRef,
  gateWorkspaceToolRef,
  type WorkspaceResourceRefGate
} from "./schemas/submission-assets.js";
import { SessionWebhookSchema } from "./schemas/session-webhook.js";
import { SessionLimitsSchema, normalizeSessionLimits } from "./schemas/session-limits.js";
import { SessionMachineSchema, normalizeSessionMachine } from "./schemas/session-machine.js";
import {
  EnvironmentSchema,
  normalizeAllowedHosts,
  normalizePlatformPackage
} from "./schemas/submission-environment.js";
import { InlineSecretsSchema, normalizeEnvSecrets } from "./schemas/submission-secrets.js";
import {
  isJsonValue,
  isRecord,
  isStringLiteral,
  type JsonValue
} from "./value-guards.js";

export type { JsonPrimitive, JsonValue } from "./value-guards.js";

/**
 * Networking + runtime-package snapshot carried inside a flat submission
 * so the hosted API can deep-clone and mutate it per session (e.g. injecting the
 * proxy hostname into `allowed_hosts`) without sharing state across
 * concurrent sessions.
 *
 * `envVars` is the customer-controlled key/value bag delivered into the
 * managed container process and mirrored in the mounted `RUNTIME.env` /
 * `RUNTIME.json` files. The same keys become `__KEY__` substitution targets
 * in agent-facing markdown inside skill / instruction / file bundles. Aex-set
 * runtime keys use the reserved `AEX_*` prefix; customer keys MUST NOT
 * collide with that prefix.
 */
export interface PlatformEnvironment {
  readonly networking?: PlatformNetworking;
  readonly packages?: readonly PlatformPackage[];
  readonly envVars?: Readonly<Record<string, string>>;
}

/**
 * Wire/input form of {@link PlatformEnvironment}, i.e. what a customer hands
 * to the SDK / sends to the submission endpoint BEFORE parsing. Identical to
 * the parsed shape except `packages` use the customer-supplied
 * {@link PlatformPackageInput} (prefixed name, no `ecosystem`). The shared
 * parser resolves it into a {@link PlatformEnvironment}.
 */
export type PlatformEnvironmentInput = Omit<PlatformEnvironment, "packages"> & {
  readonly packages?: readonly PlatformPackageInput[];
};

// Bounds, patterns and the ecosystem list live in a leaf module so the schemas
// can read them without importing this file back. Re-exported here so the
// published surface is unchanged.
export {
  AEX_RESERVED_ENV_PREFIX,
  ENV_VARS_MAX_ENTRIES,
  ENV_VARS_MAX_VALUE_BYTES,
  ENV_VARS_MAX_TOTAL_BYTES,
  PLATFORM_PACKAGE_ECOSYSTEMS
} from "./submission-limits.js";
export type { PlatformPackageEcosystem } from "./submission-limits.js";
// Also imported locally: a re-export does not bind the names in this module.
import { PLATFORM_PACKAGE_ECOSYSTEMS } from "./submission-limits.js";
import type { PlatformPackageEcosystem } from "./submission-limits.js";

export interface PlatformNetworking {
  readonly mode: "limited" | "open";
  /** Lowercase host names. The hosted API always appends the proxy host. */
  readonly allowedHosts?: readonly string[];
}

export interface PlatformPackage {
  readonly name: string;
  readonly version?: string;
  readonly ecosystem: PlatformPackageEcosystem;
}

/**
 * Submission package as the CUSTOMER supplies it on the wire: the target
 * ecosystem is encoded as an optional `name` prefix (`"pip:pandas"`,
 * `"npm:express"`, `"apt:ffmpeg"`; an unprefixed name defaults to `apt`).
 * Customers never set `ecosystem` directly — `parsePackages` resolves it and
 * the submission schema rejects `ecosystem` as an unknown field. The parsed
 * result is a {@link PlatformPackage} (bare `name` + explicit `ecosystem`).
 */
export interface PlatformPackageInput {
  readonly name: string;
  readonly version?: string;
}

const APT_PACKAGE_NAME_PATTERN = /^[a-z0-9][a-z0-9+.-]+(?::[a-z0-9][a-z0-9-]*)?$/;
const APT_EXACT_VERSION_PATTERN = /^[0-9][0-9A-Za-z.+:~-]*$/;
const NPM_PACKAGE_NAME_PATTERN = /^(?:@[a-z0-9][a-z0-9._-]*\/)?[a-z0-9][a-z0-9._-]*$/;
const NPM_EXACT_VERSION_PATTERN =
  /^(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/;
const PIP_PACKAGE_NAME_PATTERN = /^[A-Za-z0-9](?:[A-Za-z0-9._-]*[A-Za-z0-9])?$/;
const PIP_EXACT_VERSION_PATTERN = /^[0-9](?:[0-9A-Za-z.!+_-]*[0-9A-Za-z])?$/;

function assertPlatformPackage(pkg: PlatformPackage, path: string): void {
  const invalidName = () => new Error(`${path}.name must be a valid ${pkg.ecosystem} registry package name`);
  const invalidVersion = () => new Error(`${path}.version must be an exact ${pkg.ecosystem} version`);
  switch (pkg.ecosystem) {
    case "apt":
      if (!APT_PACKAGE_NAME_PATTERN.test(pkg.name)) throw invalidName();
      if (pkg.version !== undefined && !APT_EXACT_VERSION_PATTERN.test(pkg.version)) throw invalidVersion();
      return;
    case "npm":
      if (!NPM_PACKAGE_NAME_PATTERN.test(pkg.name)) throw invalidName();
      if (pkg.version !== undefined && !NPM_EXACT_VERSION_PATTERN.test(pkg.version)) throw invalidVersion();
      return;
    case "pip":
      if (!PIP_PACKAGE_NAME_PATTERN.test(pkg.name)) throw invalidName();
      if (pkg.version !== undefined && !PIP_EXACT_VERSION_PATTERN.test(pkg.version)) throw invalidVersion();
      return;
    default:
      throw new Error(`${path}.ecosystem must be one of: ${PLATFORM_PACKAGE_ECOSYSTEMS.join(", ")}`);
  }
}

/**
 * Render a parsed {@link PlatformPackage} as the version-embedded install
 * string used by runtime materialization. The join differs per manager:
 *   - pip  → `name==version`
 *   - npm  → `name@version`
 *   - apt  → `name=version`
 * With no `version`, just the bare `name`. Pure; used by the managed runner
 * package installer.
 */
export function packageInstallString(pkg: PlatformPackage): string {
  assertPlatformPackage(pkg, "package");
  if (pkg.version === undefined) {
    return pkg.name;
  }
  switch (pkg.ecosystem) {
    case "pip":
      return `${pkg.name}==${pkg.version}`;
    case "npm":
      return `${pkg.name}@${pkg.version}`;
    case "apt":
      return `${pkg.name}=${pkg.version}`;
  }
}

/**
 * A serving-provider label. Under the managed Vercel AI Gateway there is NO
 * public provider selector and no closed provider set — routing is the
 * gateway's job. This thin alias survives only because telemetry / cost /
 * custody records still carry the *serving provider string* reported by the
 * gateway's generation info (e.g. `"anthropic"`, `"deepseek"`); it is an open
 * string, never a customer input.
 */
export type ProviderName = string;

export interface PlatformMcpServerSecret {
  readonly name: string;
  readonly url: string;
  readonly headers?: Record<string, string>;
}

/**
 * Per-session inline secrets bundle. Under managed gateway keys the customer
 * supplies NO provider API keys — the platform's single managed gateway key
 * routes all model traffic. This bundle carries only non-LLM secret material:
 * `mcpServers` credentials (an MCP credential is the same secret whichever
 * model is driving the MCP client) and per-session `envSecrets`.
 */
export interface PlatformInlineSecrets {
  readonly mcpServers?: readonly PlatformMcpServerSecret[];
  /**
   * Per-session env-var secret VALUES, keyed by env name. Each entry pairs with a
   * `submission.secretEnv[<envName>] = { ephemeral: true }` declaration. Lives
   * in the secrets channel so it is vaulted and excluded from the idempotency
   * hash; the runtime injects it as the named env var and it is deleted at the
   * session's terminal. Workspace `{ ref }` bindings resolve server-side and never
   * appear here.
   */
  readonly envSecrets?: Readonly<Record<string, string>>;
}

export const SECRETS_KEY = "secrets";

/** POSIX-style env var name a `secretEnv` entry binds to (e.g. `SERPER_API_KEY`). */
export const SECRET_ENV_NAME_PATTERN = /^[A-Za-z_][A-Za-z0-9_]{0,127}$/;
/** Workspace secret handle a `secretEnv` ref points at (and the name `secret.upload` persists to). */
export const SECRET_HANDLE_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;

/**
 * One `submission.secretEnv` entry — VALUE-FREE, so it rides the (hashed)
 * submission safely. `{ ref }` resolves a workspace secret server-side;
 * `{ ephemeral: true }` pairs with a `secrets.envSecrets[<envName>]` value
 * (per-session, vaulted, deleted at the session's terminal).
 */
export type PlatformSecretEnvEntry =
  | { readonly ref: string }
  | { readonly ephemeral: true };

export const deniedSecretFields = new Set([
  "providerApiKey",
  "anthropicApiKey",
  "apiKey",
  "apiKeys",
  "accessToken",
  "refreshToken",
  "password",
  "mcpCredentials",
  "credentials"
]);

/**
 * Parse `submission.environment` — the customer-controlled runtime environment.
 *
 * The schema owns shape, the allow-list and the bounds; the normalisers own the
 * transforms (ecosystem-prefix splitting, host case folding) and the
 * collapse-to-`undefined` rules. Keeping those apart is what lets the same
 * schema generate the OpenAPI document — see D4/L1.
 */
function parseEnvironment(input: unknown): PlatformEnvironment | undefined {
  if (input === undefined) {
    return undefined;
  }
  const parsed = parseWire(EnvironmentSchema, input);
  const networking = normalizeNetworking(parsed.networking);
  const packages = normalizePackages(parsed.packages);
  const envVars = normalizeEnvVars(parsed.envVars);
  if (!networking && !packages && !envVars) {
    return undefined;
  }
  return {
    ...(networking ? { networking } : {}),
    ...(packages ? { packages } : {}),
    ...(envVars ? { envVars } : {})
  };
}

/** An empty map is treated as not supplied, so it never lands on the snapshot. */
function normalizeEnvVars(
  envVars: Record<string, unknown> | undefined
): Readonly<Record<string, string>> | undefined {
  if (envVars === undefined || Object.keys(envVars).length === 0) {
    return undefined;
  }
  return Object.freeze({ ...envVars } as Record<string, string>);
}

function normalizeNetworking(
  networking:
    | {
        readonly mode?: "limited" | "open" | undefined;
        readonly allowedHosts?: readonly string[] | undefined;
      }
    | undefined
): PlatformNetworking | undefined {
  if (networking?.mode === undefined) {
    return undefined;
  }
  const allowedHosts = networking.allowedHosts;
  return allowedHosts
    ? { mode: networking.mode, allowedHosts: normalizeAllowedHosts(allowedHosts) }
    : { mode: networking.mode };
}

function normalizePackages(
  packages: readonly { readonly name: string; readonly version?: string | undefined }[] | undefined
): readonly PlatformPackage[] | undefined {
  if (packages === undefined) {
    return undefined;
  }
  return packages.map((entry, index) => {
    const path = `submission.environment.packages[${index}]`;
    const normalized = normalizePlatformPackage(entry, path) as PlatformPackage;
    assertPlatformPackage(normalized, path);
    return normalized;
  });
}

/**
 * Cross-check `submission.secretEnv` declarations against `secrets.envSecrets`
 * values:
 *
 *  - `{ ephemeral: true }` MUST have a matching `secrets.envSecrets` value.
 *  - `{ ref }` MUST NOT supply a value (the value lives in the workspace store).
 *  - every `secrets.envSecrets` value MUST have a matching `{ ephemeral: true }`
 *    declaration (no orphan values that would never be injected).
 */
export function crossValidateSecretEnvAndValues(
  secretEnv: Readonly<Record<string, PlatformSecretEnvEntry>> | undefined,
  envSecrets: Readonly<Record<string, string>> | undefined
): void {
  const declarations = secretEnv ?? {};
  const values = envSecrets ?? {};

  for (const [envName, entry] of Object.entries(declarations)) {
    const hasValue = Object.prototype.hasOwnProperty.call(values, envName);
    if ("ref" in entry) {
      if (hasValue) {
        throw new Error(
          `submission.secretEnv[${envName}] is a workspace ref and must not supply a value in secrets.envSecrets[${envName}]; the value resolves server-side`
        );
      }
      continue;
    }
    if (!hasValue) {
      throw new Error(
        `submission.secretEnv[${envName}] is ephemeral but has no matching secrets.envSecrets[${envName}] value`
      );
    }
  }

  for (const envName of Object.keys(values)) {
    const entry = declarations[envName];
    if (!entry || !("ephemeral" in entry)) {
      throw new Error(
        `secrets.envSecrets[${envName}] has no matching submission.secretEnv[${envName}] ephemeral declaration`
      );
    }
  }
}

export function parseInlineSecrets(input: unknown): PlatformInlineSecrets {
  return withContractParseError("parseInlineSecrets", () => {
    // Absent/null secrets collapse to an empty bundle. Under managed gateway keys
    // a run needs no provider key, so an empty bundle is always admissible.
    if (input === undefined || input === null) return {};
    const parsed = parseWire(InlineSecretsSchema, input);
    const mcpServers = parsed.mcpServers as PlatformInlineSecrets["mcpServers"];
    const envSecrets = normalizeEnvSecrets(parsed.envSecrets);
    // Spread only the present halves: `PlatformInlineSecrets` promises each key
    // is absent or a value, never present-and-undefined.
    return {
      ...(mcpServers ? { mcpServers } : {}),
      ...(envSecrets ? { envSecrets } : {})
    };
  });
}

export function assertNoSecretBearingFields(input: unknown, path: readonly string[]): void {
  if (Array.isArray(input)) {
    input.forEach((item, index) => assertNoSecretBearingFields(item, [...path, String(index)]));
    return;
  }
  if (!isRecord(input)) {
    return;
  }

  for (const [key, value] of Object.entries(input)) {
    if (deniedSecretFields.has(key)) {
      throw new Error(`Secret-bearing field is not allowed in platform submission: ${[...path, key].join(".")}`);
    }
    assertNoSecretBearingFields(value, [...path, key]);
  }
}

export function requireRecord(input: unknown, field: string): Record<string, unknown> {
  if (!isRecord(input)) {
    throw new Error(`${field} must be an object`);
  }
  return input;
}

export function requireString(input: unknown, field: string): string {
  if (typeof input !== "string" || input.length === 0) {
    throw new Error(`${field} must be a non-empty string`);
  }
  return input;
}

export function optionalString(input: unknown, field: string): string | undefined {
  if (input === undefined) {
    return undefined;
  }
  return requireString(input, field);
}

export function optionalEnum<const T extends readonly string[]>(input: unknown, field: string, allowed: T): T[number] | undefined {
  if (input === undefined) {
    return undefined;
  }
  if (!isStringLiteral(input, allowed)) {
    throw new Error(`${field} must be one of: ${allowed.join(", ")}`);
  }
  return input;
}

function requireStringArray(input: unknown, field: string): readonly string[] {
  if (!Array.isArray(input) || input.length === 0 || input.some((item) => typeof item !== "string" || item.length === 0)) {
    throw new Error(`${field} must be a non-empty string array`);
  }
  return input;
}

function optionalStringRecord(input: unknown, field: string): Record<string, string> | undefined {
  if (input === undefined) {
    return undefined;
  }
  const value = requireRecord(input, field);
  for (const [key, entry] of Object.entries(value)) {
    if (typeof entry !== "string" || entry.length === 0) {
      throw new Error(`${field}.${key} must be a non-empty string`);
    }
  }
  return value as Record<string, string>;
}

function optionalJsonRecord(input: unknown, field: string): Record<string, JsonValue> | undefined {
  if (input === undefined) {
    return undefined;
  }
  const value = requireRecord(input, field);
  for (const [key, entry] of Object.entries(value)) {
    if (!isJsonValue(entry)) {
      throw new Error(`${field}.${key} must be JSON-serializable`);
    }
  }
  return value as Record<string, JsonValue>;
}

export function optionalPositiveInt(input: unknown, field: string): number | undefined {
  if (input === undefined) {
    return undefined;
  }
  if (typeof input !== "number" || !Number.isSafeInteger(input) || input <= 0) {
    throw new Error(`${field} must be a positive safe integer`);
  }
  return input;
}

/**
 * A finite positive NUMBER (fractional allowed — e.g. a USD amount like `2.5`), or
 * undefined when absent. Rejects non-numbers, NaN/Infinity, and `<= 0`.
 */
export function optionalPositiveNumber(input: unknown, field: string): number | undefined {
  if (input === undefined) {
    return undefined;
  }
  if (typeof input !== "number" || !Number.isFinite(input) || input <= 0) {
    throw new Error(`${field} must be a positive finite number`);
  }
  return input;
}

// ===========================================================================
// Session submission wire shape
// ===========================================================================

/**
 * Wire-level submission posted to /api/sessions in the flat surface. The
 * `prompt` is always an array internally so the hosted API, the audit log,
 * and the BFF idempotency hash all see one shape. `mcpServers` carries
 * only the non-secret half; bearer headers travel in
 * `secrets.mcpServers` keyed by `name`.
 *
 * Reusable resources are immutable, version-pinned refs grouped under `assets`.
 * Builtin capabilities and MCP servers remain separate submission fields.
 */
export interface PlatformSubmission {
  readonly model: ModelName;
  readonly system?: string;
  readonly prompt: readonly string[];
  /** Immutable, version-pinned workspace resources materialized for this run. */
  readonly assets: SubmissionAssets;
  readonly mcpServers: readonly McpServerRef[];
  /**
   * Env-var secret bindings — VALUE-FREE declarations keyed by env name. Each
   * value is `{ ref }` (resolve a workspace secret server-side) or
   * `{ ephemeral: true }` (value supplied in `secrets.envSecrets`, vaulted and
   * deleted at terminal). The runtime injects the resolved value as the named
   * env var. Lifecycle parity with skills/files: per-session by default, persisted
   * only when promoted to the workspace store.
   */
  readonly secretEnv?: Readonly<Record<string, PlatformSecretEnvEntry>>;
  readonly environment?: PlatformEnvironment;
  readonly securityProfile?: RuntimeSecurityProfileName;
  readonly metadata?: Record<string, JsonValue>;
  /**
   * File capture policy. Omit `fileCapture.allowedDirs` to expose regular
   * workspace files from the latest complete checkpoint; provide it to narrow
   * capture to the listed roots. `fileCapture.deniedDirs` subtracts denied
   * roots/patterns from the allowed set.
   */
  readonly fileCapture?: PlatformFileCaptureConfig;
  /** Builtin capabilities are separate from uploaded custom-tool assets. */
  readonly builtinTools: BuiltinToolsSelection;
  /**
   * Assistant-output granularity. `buffered` (the default) emits one event per
   * assistant message; `stream` emits the agent's per-token text deltas as they
   * arrive, THEN a final coalesced block. Every model streams through the
   * managed gateway, so `stream` is honored for ALL models.
   */
  readonly outputMode?: OutputMode;
  /**
   * Structured-output policy. `{ kind: 'text' }` (default) is free-form; a
   * `{ kind: 'json_schema', … }` requests provider-native constrained decode.
   * The session's typed outcome is then `decoded | refused` — no untyped path yields
   * a hallucinated object. Fail-closed for a provider lacking the capability.
   */
  readonly responseFormat?: ResponseFormat;
  /**
   * Declarative HITL write-gate: the platform parks the session
   * `awaiting_approval` BEFORE dispatching any listed tool, holding for an
   * `approve()`/`deny()`. Structural — independent of model prose. Empty/absent
   * ⇒ no gate.
   */
  readonly approvalGate?: ApprovalGate;
  /**
   * Platform-injection controls. The platform prepends a small system
   * prompt (see `platformSystemPrompt`) ahead of `system` to explain
   * managed-session expectations such as durable file capture. Set
   * `systemPrompt: "off"` to suppress that injection and have the runtime
   * see only the customer's own `system`. Omitting the field (or
   * `systemPrompt: "default"`) keeps the injection on.
   *
   * This does not change file capture scope. Omitted
   * `fileCapture.allowedDirs` means expose regular workspace files from the
   * latest complete checkpoint; explicit `fileCapture.allowedDirs` narrows it.
   */
  readonly platform?: PlatformInjectionConfig;
}

export interface PlatformFileCaptureConfig {
  /**
   * Allowed capture roots. Omit or pass an empty list to expose regular
   * workspace files from the latest complete checkpoint. Entries are absolute
   * UNIX paths.
   */
  readonly allowedDirs?: readonly string[];
  /**
   * Denied capture roots/patterns. These are subtracted from the allowed roots;
   * platform-mandatory denies always apply and cannot be re-included.
   */
  readonly deniedDirs?: readonly string[];
  /**
   * Maximum time the platform may spend capturing files after the agent exits.
   * Positive integer milliseconds; values above the platform maximum are clamped.
   */
  readonly captureTimeoutMs?: number;
  /** Maximum size of a single captured file in bytes. Positive integer. */
  readonly maxFileBytes?: number;
  /** Maximum total captured file bytes for the session. Positive integer. */
  readonly maxTotalBytes?: number;
  /** Maximum number of captured files for the session. Positive integer. */
  readonly maxFiles?: number;
}

export interface PlatformInjectionConfig {
  readonly systemPrompt?: "default" | "off";
}

export interface PlatformSessionSubmissionRequest {
  readonly workspaceId: string;
  readonly idempotencyKey: string;
  readonly submission: PlatformSubmission;
  readonly secrets: PlatformInlineSecrets;
  /**
   * Managed runtime size. One of the closed {@link RuntimeSize} preset tokens
   * or absent (downstream applies the default).
   */
  readonly runtimeSize?: RuntimeSize;
  /**
   * Execution-runtime selector — which backend runs the session
   * ({@link RuntimeKind}: `container` | `spot_container` | `lambda`). Distinct
   * from {@link runtimeSize} (the box preset). Absent ⇒ downstream applies
   * {@link import("./runtime-kind.js").DEFAULT_RUNTIME_KIND} (`lambda`).
   * `spot_container` implies interruptible capacity (reconciled with
   * {@link machine}); `lambda` is availability-gated server-side.
   */
  readonly runtimeKind?: RuntimeKind;
  /**
   * Session deadline in milliseconds, normalised by the parser from the wire
   * `timeout` duration string (bounded to [1m, 8h]). Absent ⇒
   * {@link DEFAULT_SESSION_TIMEOUT_MS} (8h). Applies to the managed runner's
   * terminal wait window and self-kill deadline.
   */
  readonly timeoutMs?: number;
  /**
   * Optional callback URL registered on this session. The platform delivers one
   * run-scoped `run.finished` or `run.error` event after each run finalizes,
   * signed Standard-Webhooks style. It is a sibling of {@link idempotencyKey} — an
   * operational/delivery concern, NOT part of the hashed submission brief, so
   * the same idempotency key with a different callback URL never 409s and the
   * field never enters `request_hash`.
   */
  readonly webhook?: SessionWebhookSpec;
  /**
   * Optional per-session override of the lineage limits (max concurrent child sessions,
   * max subagent depth, per-session spend cap). These are dials the client may
   * *request*; the server resolves them against the per-workspace ceiling and
   * the hard platform ceiling (clamping happens in the resolver, NOT this
   * parser). Absent fields fall back to the platform defaults. Only shape +
   * positivity are validated here.
   */
  readonly limits?: SessionLimits;
  /**
   * Optional capacity intent for the session's managed machine. `spot: true` opts
   * the session into interruptible capacity; absent / `spot: false` requests
   * standard capacity (the default). Intent only — the managed runtime selects
   * capacity from it.
   */
  readonly machine?: SessionMachine;
}

/** Per-session registration for finalized run callbacks; the URL must be https. */
export interface SessionWebhookSpec {
  readonly url: string;
}

/**
 * Per-session override of the lineage limits. Both fields are optional; an absent
 * field means "use the platform default". The parser ({@link parseSessionLimits})
 * only validates positivity/shape — clamping to the workspace + platform
 * ceilings is the resolver's job (see `resolveSessionLimits` in `@aexhq/shared`).
 */
export interface SessionLimits {
  readonly maxConcurrentChildSessions?: number;
  readonly maxSubagentDepth?: number;
  /**
   * Per-session spend cap in USD (defense-in-depth). The platform kills the session once
   * it would out-spend the cap. A positive number; omitted ⇒ unbounded per-session
   * (only the session's wall-clock `timeout` + the per-workspace spend cap apply).
   * Only shape/positivity are validated here.
   *
   * The frozen boot session config the managed runtime folds the loop against
   * names this same USD value `budgetUsd`; {@link sessionBudgetLimits} is the
   * single source of truth for that wire→boot name mapping.
   */
  readonly maxSpendUsd?: number;
  /**
   * Maximum number of agent ITERATIONS (turns) the session may take before the
   * platform parks it terminal. A positive integer; omitted ⇒ the platform
   * default (`SESSION_DEFAULT_MAX_TURNS`). Previously a bare server literal absent
   * from both the public contract and the limits SSoT — now a settable dial.
   * Only shape/positivity are validated here; clamping to the ceiling is the
   * resolver's job.
   */
  readonly maxTurns?: number;
  /**
   * Maximum number of agent STEPS (LLM+tool cycles) a single turn may take before
   * the platform terminalizes it — the per-turn runaway-loop backstop (doc 13 G2),
   * one level below {@link maxTurns}. A positive integer; omitted ⇒ the platform
   * default (`SESSION_DEFAULT_MAX_STEPS_PER_TURN`). Only shape/positivity are
   * validated here; clamping to the ceiling is the resolver's job.
   */
  readonly maxStepsPerTurn?: number;
}

/**
 * Per-session machine/capacity intent. v1 exposes only `spot`: opt the session into
 * interruptible capacity (`spot: true`) vs standard capacity (absent /
 * `spot: false`, the default). Only the boolean intent is public — capacity
 * selection is a runtime concern.
 */
export interface SessionMachine {
  readonly spot?: boolean;
}

/**
 * Internal pre-parser input used by authenticated platform adapters. The public
 * SDK and CLI post {@link SessionCreateRequest}; adapters derive `workspaceId`
 * and inject header idempotency before calling the platform parser.
 *
 * This type remains available from `@aexhq/contracts/internal` for platform
 * consumers and is intentionally absent from the public contracts barrel.
 */
export type PlatformSessionSubmissionInput = Omit<
  PlatformSessionSubmissionRequest,
  "workspaceId" | "timeoutMs"
> & {
  readonly workspaceId?: string;
  /**
   * Session deadline as a human duration string (`"1h"`, `"90m"`, `"30s"`).
   * Parsed + bounded to [1m, 8h] server-side into
   * {@link PlatformSessionSubmissionRequest.timeoutMs}. Absent ⇒ 8h default.
   */
  readonly timeout?: string;
};

export function parseSessionSubmissionRequest(
  input: unknown
): PlatformSessionSubmissionRequest {
  return withContractParseError("parseSessionSubmissionRequest", () => {
  const value = parseWire(SessionSubmissionRequestSchema, input);
  // Defence in depth: scan every non-secrets field for credential-named
  // keys. The `secrets` key is
  // the only allow-listed home for credential material.
  for (const [key, fieldValue] of Object.entries(value)) {
    if (key === SECRETS_KEY) {
      continue;
    }
    if (deniedSecretFields.has(key)) {
      throw new Error(`Secret-bearing field is not allowed in platform submission: ${key}`);
    }
    assertNoSecretBearingFields(fieldValue, [key]);
  }
  const runtimeSize = parseRuntimeSize(value.runtimeSize);
  const runtimeKind = parseRuntimeKind(value.runtimeKind);
  const timeoutMs = parseSessionTimeout(value.timeout);
  const webhook = parseSessionWebhook(value.webhook);
  const limits = parseSessionLimits(value.limits);
  const machine = parseSessionMachine(value.machine);
  const secrets = parseInlineSecrets(value.secrets);

  const submission = parseSubmission(value.submission);

  crossValidateSecretEnvAndValues(submission.secretEnv, secrets.envSecrets);

  // mcpServers names must agree across the submission half and the
  // secrets half — every secrets.mcpServers[i].name MUST resolve to a
  // submission.mcpServers entry (no orphan secrets) AND the URL must
  // match exactly. The reverse is allowed (an MCP server with no auth
  // headers is a valid public-MCP mode).
  if (secrets.mcpServers !== undefined) {
    const declared = new Map(submission.mcpServers.map((m) => [m.name, m.url] as const));
    for (const secret of secrets.mcpServers) {
      const declaredUrl = declared.get(secret.name);
      if (declaredUrl === undefined) {
        throw new Error(
          `secrets.mcpServers[name=${secret.name}] has no matching submission.mcpServers entry`
        );
      }
      if (declaredUrl !== secret.url) {
        throw new Error(
          `secrets.mcpServers[name=${secret.name}].url must equal submission.mcpServers[name=${secret.name}].url ` +
            `(got submission=${declaredUrl}, secrets=${secret.url})`
        );
      }
    }
  }

  return {
    workspaceId: requireString(value.workspaceId, "workspaceId"),
    idempotencyKey: requireString(value.idempotencyKey, "idempotencyKey"),
    submission,
    ...(runtimeSize ? { runtimeSize } : {}),
    ...(runtimeKind ? { runtimeKind } : {}),
    ...(timeoutMs !== undefined ? { timeoutMs } : {}),
    ...(webhook !== undefined ? { webhook } : {}),
    ...(limits !== undefined ? { limits } : {}),
    ...(machine !== undefined ? { machine } : {}),
    secrets
  };
  });
}

/**
 * Parse + SSRF-shape-validate the optional per-session `webhook`. The URL must be
 * https with no userinfo (a `user:pass@host` URL is rejected — credentials must
 * not ride in a callback URL). Unknown subfields are rejected so the strict
 * top-level allow-list extends to the nested object. Returns `undefined` when
 * absent. Delivery-time re-resolution + IP-deny checks live server-side; this
 * is the submit-time shape gate.
 */
export function parseSessionWebhook(input: unknown): SessionWebhookSpec | undefined {
  return withContractParseError("parseSessionWebhook", () => {
    if (input === undefined) {
      return undefined;
    }
    return parseWire(SessionWebhookSchema, input);
  });
}

/**
 * Parse the optional per-session `limits` override. Mirrors {@link parseSessionWebhook}:
 * absent ⇒ `undefined`; a non-object or any unknown subfield is rejected so the
 * strict top-level allow-list extends to the nested object. Each present field
 * is validated as a positive safe integer via {@link optionalPositiveInt}.
 *
 * This is a SHAPE/positivity gate only — it does NOT clamp to the workspace or
 * platform ceilings (that precedence lives in the resolver, `resolveSessionLimits`).
 * Only the present fields are returned; an all-absent override (e.g. `{}`)
 * collapses to `undefined` so it carries no signal onto the request.
 */
export function parseSessionLimits(input: unknown): SessionLimits | undefined {
  return withContractParseError("parseSessionLimits", () => {
    if (input === undefined) {
      return undefined;
    }
    return normalizeSessionLimits(parseWire(SessionLimitsSchema, input));
  });
}

/**
 * Boot-session budget fragment. The public submit surface names a session's spend
 * cap `limits.maxSpendUsd`; the frozen boot session config the managed runtime
 * folds the loop against names the SAME USD value `budgetUsd` — the field the
 * session planner reads to enforce/terminate a session that would out-spend its cap.
 * This is the single source of truth for that wire→boot name mapping so the two
 * layers can never drift.
 *
 * Returns a fragment safe to spread into `sessionConfig.limits`: `{ budgetUsd }`
 * when a cap is set, `{}` when none is (an absent cap stays absent — the session is
 * unbounded per-session, subject only to the session timeout + the per-workspace cap).
 * Pure: same input ⇒ same output.
 */
export function sessionBudgetLimits(limits: SessionLimits | undefined): { budgetUsd?: number } {
  if (limits?.maxSpendUsd === undefined) {
    return {};
  }
  return { budgetUsd: limits.maxSpendUsd };
}

/**
 * Parse the optional per-session `machine` capacity intent. Mirrors
 * {@link parseSessionWebhook}: absent ⇒ `undefined`; a non-object or any unknown
 * subfield is rejected so the strict top-level allow-list extends to the nested
 * object. `spot` must be a boolean when present. A no-signal object (e.g.
 * `machine: {}`) collapses to `undefined` so it never lands an empty object on
 * the request. An explicit `spot` (true or false) is preserved verbatim. Only
 * shape is validated here — capacity selection is a runtime concern.
 */
export function parseSessionMachine(input: unknown): SessionMachine | undefined {
  return withContractParseError("parseSessionMachine", () => {
    if (input === undefined) {
      return undefined;
    }
    return normalizeSessionMachine(parseWire(SessionMachineSchema, input));
  });
}

export function parseSubmission(input: unknown): PlatformSubmission {
  return withContractParseError("parseSubmission", () => {
  const value = parseWire(SubmissionSchema, input);
  const model = parseModelSlug(value.model, "submission.model");
  const system = optionalString(value.system, "submission.system");
  const prompt = parsePrompt(value.prompt);
  const assets = parseSubmissionAssets(value.assets);
  const mcpServers = parseMcpServers(value.mcpServers);
  const secretEnv = parseSecretEnv(value.secretEnv);
  const environment = parseEnvironment(value.environment);
  const securityProfile = parseRuntimeSecurityProfile(value.securityProfile);
  const metadata = optionalJsonRecord(value.metadata, "submission.metadata");
  const fileCapture = parseFileCapture(value.fileCapture);
  const builtinTools = parseBuiltinToolsSelection(value.builtinTools);
  const outputMode = parseOutputMode(value.outputMode);
  const responseFormat = parseResponseFormat(value.responseFormat);
  const approvalGate = parseApprovalGate(value.approvalGate);
  const platform = parsePlatformConfig(value.platform);

  return {
    model,
    ...(system ? { system } : {}),
    prompt,
    assets,
    mcpServers,
    ...(secretEnv ? { secretEnv } : {}),
    ...(environment ? { environment } : {}),
    ...(securityProfile ? { securityProfile } : {}),
    ...(metadata ? { metadata } : {}),
    ...(fileCapture ? { fileCapture } : {}),
    builtinTools,
    ...(outputMode !== undefined ? { outputMode } : {}),
    ...(responseFormat !== undefined ? { responseFormat } : {}),
    ...(approvalGate !== undefined ? { approvalGate } : {}),
    ...(platform ? { platform } : {})
  };
  });
}

function parseSubmissionAssets(input: unknown): SubmissionAssets {
  const value = parseWire(SubmissionAssetsSchema, input);
  return {
    files: parseWorkspaceFiles(value.files),
    skills: parseWorkspaceSkills(value.skills),
    tools: parseWorkspaceTools(value.tools),
    instructions: parseWorkspaceInstructions(value.instructions)
  };
}

function parseWorkspaceFiles(input: unknown): readonly WorkspaceFileRef[] {
  return parseWorkspaceResourceArray(
    input,
    "files",
    "file",
    gateWorkspaceFileRef,
    (raw, base, path) => {
    const name = requireString(raw.name, `${path}.name`);
    assertWorkspaceFileResourceName(name, `${path}.name`);
    const mountPath = requireString(raw.mountPath, `${path}.mountPath`);
    assertValidMountPath(mountPath, `${path}.mountPath`);
    return { ...base, kind: "file", name, mountPath };
    }
  );
}

function parseWorkspaceSkills(input: unknown): readonly WorkspaceSkillRef[] {
  return parseWorkspaceResourceArray(
    input,
    "skills",
    "skill",
    gateWorkspaceSkillRef,
    (raw, base, path) => {
    const name = requireString(raw.name, `${path}.name`);
    assertValidSkillName(name, `${path}.name`);
    const description = requireResourceDescription(raw.description, `${path}.description`);
    return { ...base, kind: "skill", name, description };
    }
  );
}

function parseWorkspaceTools(input: unknown): readonly WorkspaceToolRef[] {
  return parseWorkspaceResourceArray(
    input,
    "tools",
    "tool",
    gateWorkspaceToolRef,
    (raw, base, path) => {
      const name = requireString(raw.name, `${path}.name`);
      if (!TOOL_NAME_PATTERN.test(name) || name.includes("__")) {
        throw new Error(`${path}.name must be a non-reserved tool name matching ${TOOL_NAME_PATTERN.source}`);
      }
      const description = requireResourceDescription(raw.description, `${path}.description`);
      const inputSchema = requireRecord(raw.input_schema, `${path}.input_schema`);
      if (!isJsonValue(inputSchema) || inputSchema.type !== "object") {
        throw new Error(`${path}.input_schema must be a JSON Schema object with type 'object'`);
      }
      const entry = normaliseSkillBundlePath(requireString(raw.entry, `${path}.entry`));
      return {
        ...base,
        kind: "tool",
        name,
        description,
        input_schema: inputSchema as ToolInputSchema,
        entry
      };
    }
  );
}

function parseWorkspaceInstructions(input: unknown): readonly WorkspaceInstructionRef[] {
  return parseWorkspaceResourceArray(
    input,
    "instructions",
    "instruction",
    gateWorkspaceInstructionRef,
    (raw, base, path) => {
    const name = requireString(raw.name, `${path}.name`);
    assertWorkspaceInstructionResourceName(name, `${path}.name`);
    return { ...base, kind: "instruction", name };
    }
  );
}

type PinnedResourceBase = Pick<
  WorkspaceFileRef,
  "resourceId" | "version" | "assetId" | "contentHash"
>;

function parseWorkspaceResourceArray<T extends WorkspaceFileRef | WorkspaceSkillRef | WorkspaceToolRef | WorkspaceInstructionRef>(
  input: unknown,
  field: "files" | "skills" | "tools" | "instructions",
  kind: T["kind"],
  gateKeys: WorkspaceResourceRefGate,
  project: (raw: Record<string, unknown>, base: PinnedResourceBase, path: string) => T
): readonly T[] {
  if (input === undefined) return [];
  if (!Array.isArray(input)) throw new Error(`submission.assets.${field} must be an array`);
  const seen = new Set<string>();
  return input.map((item, index) => {
    const path = `submission.assets.${field}[${index}]`;
    const raw = gateKeys(path, item);
    if (raw.kind !== kind) throw new Error(`${path}.kind must be '${kind}'`);
    const base: PinnedResourceBase = {
      resourceId: requireString(raw.resourceId, `${path}.resourceId`),
      version: requirePositiveInteger(raw.version, `${path}.version`),
      assetId: requireString(raw.assetId, `${path}.assetId`),
      contentHash: requireString(raw.contentHash, `${path}.contentHash`)
    };
    const result = project(raw, base, path);
    assertPinnedWorkspaceResource(result, path);
    const identity = `${result.resourceId}:${result.version}`;
    if (seen.has(identity)) throw new Error(`${path} duplicates resource version ${identity}`);
    seen.add(identity);
    return result;
  });
}

function requirePositiveInteger(input: unknown, path: string): number {
  if (!Number.isSafeInteger(input) || (input as number) < 1) {
    throw new Error(`${path} must be a positive integer`);
  }
  return input as number;
}

function requireResourceDescription(input: unknown, path: string): string {
  const value = requireString(input, path);
  if (value.trim().length === 0 || value.length > 2048) {
    throw new Error(`${path} must be non-empty and <= 2048 chars`);
  }
  return value;
}

function parseSecretEnv(
  input: unknown
): Readonly<Record<string, PlatformSecretEnvEntry>> | undefined {
  if (input === undefined || input === null) return undefined;
  const value = requireRecord(input, "submission.secretEnv");
  const out: Record<string, PlatformSecretEnvEntry> = {};
  for (const [envName, entry] of Object.entries(value)) {
    if (!SECRET_ENV_NAME_PATTERN.test(envName)) {
      throw new Error(
        `submission.secretEnv key "${envName}" must be a valid env var name matching ${SECRET_ENV_NAME_PATTERN.source}`
      );
    }
    const path = `submission.secretEnv.${envName}`;
    const record = requireRecord(entry, path);
    const keys = Object.keys(record);
    if (keys.length !== 1 || (!("ref" in record) && !("ephemeral" in record))) {
      throw new Error(`${path} must be exactly one of { ref } or { ephemeral: true }`);
    }
    if ("ref" in record) {
      const handle = requireString(record.ref, `${path}.ref`);
      if (!SECRET_HANDLE_PATTERN.test(handle)) {
        throw new Error(`${path}.ref handle must match ${SECRET_HANDLE_PATTERN.source}`);
      }
      out[envName] = { ref: handle };
    } else {
      if (record.ephemeral !== true) {
        throw new Error(`${path}.ephemeral must be the literal true`);
      }
      out[envName] = { ephemeral: true };
    }
  }
  return Object.keys(out).length > 0 ? out : undefined;
}

function parsePlatformConfig(input: unknown): PlatformInjectionConfig | undefined {
  if (input === undefined || input === null) return undefined;
  const value = parseWire(PlatformInjectionSchema, input);
  if (value.systemPrompt === undefined) return undefined;
  if (value.systemPrompt !== "default" && value.systemPrompt !== "off") {
    throw new Error(`submission.platform.systemPrompt must be "default" or "off"`);
  }
  return { systemPrompt: value.systemPrompt };
}

/** Assistant-output granularity values. Buffered is the platform default. */
export const OUTPUT_MODES = ["buffered", "stream"] as const;
export type OutputMode = (typeof OUTPUT_MODES)[number];
export const DEFAULT_OUTPUT_MODE: OutputMode = "buffered";

function parseOutputMode(input: unknown): OutputMode | undefined {
  if (input === undefined || input === null) return undefined;
  if (typeof input !== "string" || !(OUTPUT_MODES as readonly string[]).includes(input)) {
    throw new Error(`submission.outputMode must be one of ${OUTPUT_MODES.join(", ")}`);
  }
  return input as OutputMode;
}

// ---------------------------------------------------------------------------
// Structured-output (schema-decode) policy — WS10.
// ---------------------------------------------------------------------------

/** Response-format kinds: free-form `text` (default) or provider-native `json_schema`. */
export const RESPONSE_FORMAT_KINDS = ["text", "json_schema"] as const;
export type ResponseFormatKind = (typeof RESPONSE_FORMAT_KINDS)[number];

/**
 * Structured-output policy. `{ kind: 'text' }` is free-form; `{ kind:
 * 'json_schema', schema, strict?, name? }` requests provider-native constrained
 * decode against `schema` (a JSON Schema object).
 */
export type ResponseFormat =
  | { readonly kind: "text" }
  | {
      readonly kind: "json_schema";
      readonly schema: JsonValue;
      readonly strict?: boolean;
      readonly name?: string;
    };

/**
 * Parse the optional `submission.responseFormat`. Mirrors {@link parseOutputMode}
 * / {@link OUTPUT_MODES}: absent ⇒ undefined; a bad `kind` or unknown subfield is
 * rejected (fail-fast). `json_schema` requires a JSON-object `schema`.
 */
export function parseResponseFormat(input: unknown): ResponseFormat | undefined {
  return withContractParseError("parseResponseFormat", () => {
  if (input === undefined || input === null) return undefined;
  const value = requireRecord(input, "submission.responseFormat");
  const kind = value.kind;
  if (typeof kind !== "string" || !(RESPONSE_FORMAT_KINDS as readonly string[]).includes(kind)) {
    throw new Error(`submission.responseFormat.kind must be one of ${RESPONSE_FORMAT_KINDS.join(", ")}`);
  }
  // `kind` selects the variant BEFORE either schema runs — a bad `kind` must
  // outrank an unknown key, and the two variants word their rejection
  // differently — so the object gate above stays a `requireRecord` and each
  // schema below is a pure key gate over the record it already produced.
  if (kind === "text") {
    parseWire(ResponseFormatTextSchema, value);
    return { kind: "text" };
  }
  parseWire(ResponseFormatJsonSchemaSchema, value);
  if (!isRecord(value.schema) || !isJsonValue(value.schema)) {
    throw new Error("submission.responseFormat.schema must be a JSON-serializable object");
  }
  const schema = value.schema as JsonValue;
  if (value.strict !== undefined && typeof value.strict !== "boolean") {
    throw new Error("submission.responseFormat.strict must be a boolean");
  }
  const name = optionalString(value.name, "submission.responseFormat.name");
  return {
    kind: "json_schema",
    schema,
    ...(value.strict !== undefined ? { strict: value.strict } : {}),
    ...(name !== undefined ? { name } : {})
  };
  });
}

// ---------------------------------------------------------------------------
// HITL approval-gate policy — WS10.
// ---------------------------------------------------------------------------

/** Declarative HITL write-gate: park `awaiting_approval` before any listed tool. */
export interface ApprovalGate {
  readonly tools: readonly string[];
}

/**
 * Parse the optional `submission.approvalGate`. Absent / empty tool list ⇒
 * undefined (no gate). Tool names are deduped; the strict allow-list mirrors the
 * sibling parsers.
 */
export function parseApprovalGate(input: unknown): ApprovalGate | undefined {
  return withContractParseError("parseApprovalGate", () => {
  if (input === undefined || input === null) return undefined;
  const value = parseWire(ApprovalGateSchema, input);
  if (!Array.isArray(value.tools)) {
    throw new Error("submission.approvalGate.tools must be an array of tool names");
  }
  const seen = new Set<string>();
  const tools: string[] = [];
  value.tools.forEach((entry, index) => {
    if (typeof entry !== "string" || entry.length === 0) {
      throw new Error(`submission.approvalGate.tools[${index}] must be a non-empty string`);
    }
    if (!seen.has(entry)) {
      seen.add(entry);
      tools.push(entry);
    }
  });
  if (tools.length === 0) return undefined;
  return { tools };
  });
}

/**
 * The CLOSED set of builtin tool NAMES the managed runtime can inject — one per
 * machine tool the hands implement. This list is the single source of truth for
 * validating builtin tool references; the platform's `HANDS_TOOLS` (the execute
 * vocabulary) is pinned EQUAL to it at module load (`platform-runtime-agent`
 * `assertNamesMatch`), so a rename on either side fails loudly rather than
 * silently shipping a name the executors do not speak.
 *
 * Order mirrors `HANDS_TOOLS`. A builtin tool reference (a bare string in
 * `submission.tools`) must be a member of this set.
 */
export const BUILTIN_TOOL_NAMES = [
  "bash",
  "read_file",
  "write_file",
  "edit_file",
  "grep",
  "glob",
  "head",
  "tail",
  "todo_write",
  "subagent",
  "subagent_result",
  "web_fetch",
  "web_search",
  "bash_output",
  "bash_kill",
  "code_execution",
  "wait",
  "git",
  "ls",
  "stat",
  "wc"
] as const;
export type BuiltinToolName = (typeof BUILTIN_TOOL_NAMES)[number];

/**
 * Typo-safe accessors for the closed builtin tool set: each key maps to the
 * real tool NAME string. Reference a builtin in `submission.tools` via
 * `BuiltinTools.web_search` rather than the bare string so a rename is a
 * compile error, not a runtime 400.
 *
 * Keys are the real tool names; a unit test asserts `Object.values(BuiltinTools)`
 * deep-equals `BUILTIN_TOOL_NAMES` so the two can never drift.
 */
export const BuiltinTools = {
  bash: "bash",
  read_file: "read_file",
  write_file: "write_file",
  edit_file: "edit_file",
  grep: "grep",
  glob: "glob",
  head: "head",
  tail: "tail",
  todo_write: "todo_write",
  subagent: "subagent",
  subagent_result: "subagent_result",
  web_fetch: "web_fetch",
  web_search: "web_search",
  bash_output: "bash_output",
  bash_kill: "bash_kill",
  code_execution: "code_execution",
  wait: "wait",
  git: "git",
  ls: "ls",
  stat: "stat",
  wc: "wc"
} as const satisfies Readonly<Record<BuiltinToolName, BuiltinToolName>>;

/**
 * The complete builtin set selected by `builtinTools: "default"`.
 */
export const DEFAULT_BUILTIN_TOOLS: readonly BuiltinToolName[] = BUILTIN_TOOL_NAMES;

// ---------------------------------------------------------------------------
// The default `skills` meta-tool
// ---------------------------------------------------------------------------

/**
 * Fixed name of the single skills meta-tool. Deliberately NOT a member of
 * {@link BUILTIN_TOOL_NAMES} (that closed set is the customer-cherry-pickable
 * toggle surface, pinned equal to `HANDS_TOOLS`) — the skills tool is IMPLIED by
 * a session having ≥1 skill, not chosen, and is injected platform-side. Kept in
 * lockstep with {@link SKILL_RESERVED_NAMES} so it can never be shadowed by a
 * custom tool or skill of the same name.
 */
export const SKILLS_TOOL_NAME = "skills";

/**
 * The single default `skills` meta-tool (list/load) the platform injects when a
 * session references ≥1 workspace skill. Shared by the platform tool composer and
 * kept adjacent to the reserved-name guard so the model-visible contract and the
 * name reservation stay in one place. It replaces the former N per-skill no-arg
 * load-tools with one arg-taking dispatcher.
 */
export const SKILLS_TOOL_DEFINITION = {
  name: "skills",
  description:
    "List and load the workspace SKILLS available to this session. Call with {action:'list'} to see each skill's " +
    "name + description (cheap; do this first). Call with {action:'load', name:'<skill>'} to read that skill's " +
    "full SKILL.md instructions into context before doing work the skill governs. A skill's supporting files are " +
    "already on disk under /workspace/skills/<name>/ — load pulls the instructions; read_file/bash read the rest.",
  input_schema: {
    type: "object",
    properties: {
      action: { type: "string", enum: ["list", "load"], description: "'list' all skills, or 'load' one by name." },
      name: { type: "string", description: "Skill name to load (required when action='load')." }
    },
    required: ["action"],
    additionalProperties: false
  }
} as const;

/**
 * Resolve the set of builtin tool NAMES a submission injects, deduplicated and
 * in {@link BUILTIN_TOOL_NAMES} order.
 *
 * `"default"` selects the standard set, `"none"` selects none, and an array
 * selects a validated subset in canonical order.
 */
export type BuiltinToolsSelection = "default" | "none" | readonly BuiltinToolName[];

export function resolveBuiltinToolNames(
  selection: BuiltinToolsSelection = "default"
): readonly BuiltinToolName[] {
  if (selection === "default") return DEFAULT_BUILTIN_TOOLS;
  if (selection === "none") return [];
  const enabled = new Set<string>();
  for (const ref of selection) {
    if (!(BUILTIN_TOOL_NAMES as readonly string[]).includes(ref)) {
      throw new Error(
        `${JSON.stringify(ref)} is not a builtin tool; expected one of: ${BUILTIN_TOOL_NAMES.join(", ")}`
      );
    }
    enabled.add(ref);
  }
  return BUILTIN_TOOL_NAMES.filter((name) => enabled.has(name));
}

function parseBuiltinToolsSelection(input: unknown): BuiltinToolsSelection {
  if (input === undefined || input === null || input === "default") return "default";
  if (input === "none") return "none";
  if (!Array.isArray(input)) {
    throw new Error("submission.builtinTools must be 'default', 'none', or an array of builtin tool names");
  }
  return resolveBuiltinToolNames(input.map((value, index) => {
    if (typeof value !== "string") {
      throw new Error(`submission.builtinTools[${index}] must be a builtin tool name`);
    }
    return value as BuiltinToolName;
  }));
}

/**
 * Maximum number of file capture entries accepted per list.
 *
 * 32 is enough room for the typical "one or two capture roots" pattern
 * plus a generous margin for legitimate multi-root use cases (per-tool
 * file directory + scratch state + logs, repeated across a few
 * subdirectories), without inviting abuse of the synthetic-turn path
 * the platform capture path drives at session terminal.
 */
const MAX_FILE_CAPTURE_DIRS = 32;

/**
 * Maximum byte length of a single file capture entry (after UTF-8
 * encoding). 512 bytes comfortably covers `/very/long/nested/path`
 * style entries without letting a misuse smuggle large blobs through
 * the field.
 */
const MAX_FILE_CAPTURE_DIR_BYTES = 512;
const MAX_FILE_CAPTURE_TIMEOUT_MS = 6 * 60 * 60 * 1000;

function parseFileCapture(input: unknown): PlatformFileCaptureConfig | undefined {
  if (input === undefined || input === null) {
    return undefined;
  }
  const value = parseWire(FileCaptureSchema, input);
  const allowedDirs = parseFileCaptureAllowedDirs(value.allowedDirs);
  const deniedDirs = parseFileCaptureDeniedDirs(value.deniedDirs);
  const captureTimeoutMs = parseFileCapturePositiveInteger(value.captureTimeoutMs, "submission.fileCapture.captureTimeoutMs", {
    max: MAX_FILE_CAPTURE_TIMEOUT_MS,
    clamp: true
  });
  const maxFileBytes = parseFileCapturePositiveInteger(value.maxFileBytes, "submission.fileCapture.maxFileBytes");
  const maxTotalBytes = parseFileCapturePositiveInteger(value.maxTotalBytes, "submission.fileCapture.maxTotalBytes");
  const maxFiles = parseFileCapturePositiveInteger(value.maxFiles, "submission.fileCapture.maxFiles");
  if (!allowedDirs && !deniedDirs && captureTimeoutMs === undefined && maxFileBytes === undefined && maxTotalBytes === undefined && maxFiles === undefined) {
    return undefined;
  }
  return {
    ...(allowedDirs ? { allowedDirs } : {}),
    ...(deniedDirs ? { deniedDirs } : {}),
    ...(captureTimeoutMs !== undefined ? { captureTimeoutMs } : {}),
    ...(maxFileBytes !== undefined ? { maxFileBytes } : {}),
    ...(maxTotalBytes !== undefined ? { maxTotalBytes } : {}),
    ...(maxFiles !== undefined ? { maxFiles } : {})
  };
}

function parseFileCapturePositiveInteger(
  input: unknown,
  field: string,
  options: { readonly max?: number; readonly clamp?: boolean } = {}
): number | undefined {
  if (input === undefined) {
    return undefined;
  }
  if (typeof input !== "number" || !Number.isInteger(input) || input <= 0) {
    throw new Error(`${field} must be a positive integer`);
  }
  if (options.max !== undefined && input > options.max) {
    return options.clamp ? options.max : input;
  }
  return input;
}

function parseFileCaptureAllowedDirs(input: unknown): readonly string[] | undefined {
  if (input === undefined) {
    return undefined;
  }
  if (!Array.isArray(input)) {
    throw new Error("submission.fileCapture.allowedDirs must be an array of absolute UNIX paths");
  }
  if (input.length === 0) {
    // Treat an empty array as omission so the idempotency hash matches
    // the "no allowedDirs" case.
    return undefined;
  }
  if (input.length > MAX_FILE_CAPTURE_DIRS) {
    throw new Error(
      `submission.fileCapture.allowedDirs has ${input.length} entries; max is ${MAX_FILE_CAPTURE_DIRS}`
    );
  }
  const seen = new Set<string>();
  const normalised: string[] = [];
  for (let i = 0; i < input.length; i++) {
    const item = input[i];
    if (typeof item !== "string") {
      throw new Error(`submission.fileCapture.allowedDirs[${i}] must be a string`);
    }
    if (item.length === 0) {
      throw new Error(`submission.fileCapture.allowedDirs[${i}] must be a non-empty absolute UNIX path`);
    }
    const bytes = new TextEncoder().encode(item).length;
    if (bytes > MAX_FILE_CAPTURE_DIR_BYTES) {
      throw new Error(
        `submission.fileCapture.allowedDirs[${i}] exceeds ${MAX_FILE_CAPTURE_DIR_BYTES} bytes (got ${bytes})`
      );
    }
    if (!item.startsWith("/")) {
      throw new Error(
        `submission.fileCapture.allowedDirs[${i}] must be an absolute UNIX path (start with '/')`
      );
    }
    if (item.includes("\0")) {
      throw new Error(`submission.fileCapture.allowedDirs[${i}] must not contain NUL bytes`);
    }
    if (item.includes("\n") || item.includes("\r")) {
      throw new Error(`submission.fileCapture.allowedDirs[${i}] must not contain newline characters`);
    }
    const segments = item.split("/");
    if (segments.includes("..")) {
      throw new Error(`submission.fileCapture.allowedDirs[${i}] must not contain '..' segments`);
    }
    const collapsed = segments
      .filter((seg, idx) => seg.length > 0 || idx === 0)
      .join("/");
    const stripped =
      collapsed.length > 1 && collapsed.endsWith("/")
        ? collapsed.slice(0, -1)
        : collapsed;
    const canonical = stripped.length === 0 ? "/" : stripped;
    if (seen.has(canonical)) {
      continue;
    }
    seen.add(canonical);
    normalised.push(canonical);
  }
  return normalised;
}

function parseFileCaptureDeniedDirs(input: unknown): readonly string[] | undefined {
  if (input === undefined) {
    return undefined;
  }
  if (!Array.isArray(input)) {
    throw new Error("submission.fileCapture.deniedDirs must be an array of strings");
  }
  if (input.length === 0) {
    return undefined;
  }
  if (input.length > MAX_FILE_CAPTURE_DIRS) {
    throw new Error(`submission.fileCapture.deniedDirs has ${input.length} entries; max is ${MAX_FILE_CAPTURE_DIRS}`);
  }
  const seen = new Set<string>();
  const normalised: string[] = [];
  for (let i = 0; i < input.length; i++) {
    const item = input[i];
    if (typeof item !== "string") {
      throw new Error(`submission.fileCapture.deniedDirs[${i}] must be a string`);
    }
    if (item.length === 0) {
      throw new Error(`submission.fileCapture.deniedDirs[${i}] must be a non-empty pattern`);
    }
    const bytes = new TextEncoder().encode(item).length;
    if (bytes > MAX_FILE_CAPTURE_DIR_BYTES) {
      throw new Error(`submission.fileCapture.deniedDirs[${i}] exceeds ${MAX_FILE_CAPTURE_DIR_BYTES} bytes (got ${bytes})`);
    }
    if (item.includes("\0")) {
      throw new Error(`submission.fileCapture.deniedDirs[${i}] must not contain NUL bytes`);
    }
    if (item.includes("\n") || item.includes("\r")) {
      throw new Error(`submission.fileCapture.deniedDirs[${i}] must not contain newline characters`);
    }
    if (item.split("/").includes("..")) {
      throw new Error(`submission.fileCapture.deniedDirs[${i}] must not contain '..' segments`);
    }
    let canonical = item;
    if (item.startsWith("/")) {
      const collapsed = item
        .split("/")
        .filter((seg, idx) => seg.length > 0 || idx === 0)
        .join("/");
      canonical =
        collapsed.length > 1 && collapsed.endsWith("/") ? collapsed.slice(0, -1) : collapsed;
    }
    if (seen.has(canonical)) {
      continue;
    }
    seen.add(canonical);
    normalised.push(canonical);
  }
  return normalised;
}

function parsePrompt(input: unknown): readonly string[] {
  if (typeof input === "string") {
    if (input.length === 0) {
      throw new Error("submission.prompt must be non-empty");
    }
    if (input.trim().length === 0) {
      throw new Error("submission.prompt must contain non-whitespace text");
    }
    return [input];
  }
  if (!Array.isArray(input)) {
    throw new Error("submission.prompt must be a string or an array of strings");
  }
  if (input.length === 0) {
    throw new Error("submission.prompt array must be non-empty");
  }
  const parts = input.map((item, index) => {
    if (typeof item !== "string" || item.length === 0) {
      throw new Error(`submission.prompt[${index}] must be a non-empty string`);
    }
    return item;
  });
  if (parts.every((part) => part.trim().length === 0)) {
    throw new Error("submission.prompt must contain non-whitespace text");
  }
  return parts;
}

/**
 * Shared skill-name gate for {@link parseSkills}
 * (and mirrored SDK-side in `Skill`): pattern + `__` MCP separator + reserved
 * names (`skills`, `skill`).
 */
function assertValidSkillName(name: string, field: string): void {
  if (!SKILL_NAME_PATTERN.test(name)) {
    throw new Error(`${field} must match ${SKILL_NAME_PATTERN.source}`);
  }
  if (name.includes("__")) {
    throw new Error(`${field} must not contain "__"; that separator is reserved for MCP tools`);
  }
  if (SKILL_RESERVED_NAMES.has(name)) {
    throw new Error(`${field} must not be a reserved skills name (${[...SKILL_RESERVED_NAMES].join(", ")})`);
  }
}

function parseMcpServers(input: unknown): readonly McpServerRef[] {
  if (input === undefined) {
    return [];
  }
  if (!Array.isArray(input)) {
    throw new Error("submission.mcpServers must be an array of {name, url} objects");
  }
  const seen = new Set<string>();
  return input.map((item, index) => {
    const ref = parseMcpServerRef(item, `submission.mcpServers[${index}]`);
    if (seen.has(ref.name)) {
      throw new Error(`submission.mcpServers duplicate name: ${ref.name}`);
    }
    seen.add(ref.name);
    return ref;
  });
}

