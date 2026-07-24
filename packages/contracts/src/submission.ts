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
  assertModelNameMatchesProvider,
  parseModelName,
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
import { assertAllowedKeys, defineAllowedKeys } from "./allowed-keys.js";
import { UnknownFieldError } from "./unknown-field-error.js";
import { withContractParseError } from "./contract-parse-error.js";
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

/**
 * Reserved prefix for aex-set runtime env vars (`AEX_CLI`,
 * `AEX_RUNTIME_JSON`, …). Customer `environment.envVars` keys carrying this
 * prefix are rejected at submission parse time so platform-set values
 * cannot be silently overwritten.
 */
export const AEX_RESERVED_ENV_PREFIX = "AEX_";

/**
 * Maximum number of `environment.envVars` entries accepted per
 * submission. Picked to be generous for real customer config bags
 * (the broll case ships a handful — `BROLL_STORE`, `BROLL_OUTPUTS`,
 * `BROLL_MODE`, …) while still bounding the size of every RUNTIME
 * file we mount into the container.
 */
export const ENV_VARS_MAX_ENTRIES = 64;

/** Maximum byte length of a single `environment.envVars` value. */
export const ENV_VARS_MAX_VALUE_BYTES = 4096;

/** Maximum total byte length of all `environment.envVars` keys+values combined. */
export const ENV_VARS_MAX_TOTAL_BYTES = 65536;

/**
 * POSIX-shell-portable env var key: starts with `A-Z` or `_`, body is
 * `A-Z`, `0-9`, `_`. We deliberately reject lowercase to keep
 * `RUNTIME.env` readable and consistent with platform conventions; if
 * a customer has lowercase keys today, they uppercase them at the
 * call site.
 */
const ENV_VAR_KEY_PATTERN = /^[A-Z_][A-Z0-9_]*$/;

export interface PlatformNetworking {
  readonly mode: "limited" | "open";
  /** Lowercase host names. The hosted API always appends the proxy host. */
  readonly allowedHosts?: readonly string[];
}

/**
 * Package-manager ecosystems accepted by the public submission schema.
 * The customer encodes the target manager as a `name` prefix
 * `"<eco>:<pkg>"` (e.g. "pip:pandas", "npm:express", "apt:ffmpeg"); an
 * UNPREFIXED name defaults to `apt`. After parsing, `PlatformPackage.name`
 * is the bare package and `PlatformPackage.ecosystem` is the resolved
 * manager.
 */
export const PLATFORM_PACKAGE_ECOSYSTEMS = ["apt", "npm", "pip"] as const;
export type PlatformPackageEcosystem = (typeof PLATFORM_PACKAGE_ECOSYSTEMS)[number];

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
 * Submission-time provider selector. Aex exposes one customer interface
 * for every provider. All new submissions execute through the managed
 * runtime; provider selection only decides which upstream model route
 * the managed provider-proxy uses.
 */
export const PROVIDERS = [
  "anthropic",
  "deepseek",
  "openai",
  "gemini",
  "mistral",
  "openrouter",
  "doubao"
] as const;
export type ProviderName = (typeof PROVIDERS)[number];
export const DEFAULT_PROVIDER: ProviderName = "anthropic";

/**
 * Symbol-style accessors for the closed provider set. Prefer these over raw
 * strings so an invalid token is a compile error, not a runtime 400 — e.g.
 * `Providers.DEEPSEEK`. The same model id can route through different upstream
 * providers (official vs OpenRouter, etc.), so `provider` is a first-class
 * submission field; name it explicitly with one of these constants rather than
 * relying on the model alone to determine routing.
 *
 * Every value mirrors {@link PROVIDERS} exactly; a unit test asserts
 * `Object.values(Providers)` deep-equals `PROVIDERS` so the two can never
 * drift.
 */
export const Providers = {
  /** Anthropic — Claude models. */
  ANTHROPIC: "anthropic",
  /** DeepSeek. */
  DEEPSEEK: "deepseek",
  /** OpenAI — GPT models. */
  OPENAI: "openai",
  /** Google Gemini. */
  GEMINI: "gemini",
  /** Mistral. */
  MISTRAL: "mistral",
  /** OpenRouter — OpenAI-compatible aggregator routing to many upstream models. */
  OPENROUTER: "openrouter",
  /** Doubao (ByteDance) via the official international BytePlus ModelArk gateway. */
  DOUBAO: "doubao"
} as const satisfies Readonly<Record<string, ProviderName>>;

export interface PlatformMcpServerSecret {
  readonly name: string;
  readonly url: string;
  readonly headers?: Record<string, string>;
}

/**
 * Per-session inline secrets bundle. `apiKeys` holds the BYOK provider keys, keyed
 * by {@link ProviderName}. A session REQUIRES a key for its own `provider`; it MAY
 * carry keys for additional providers so a subagent spawned with a
 * different-family model inherits them server-side from the vault (the keys
 * never transit the container). `mcpServers` credentials are cross-provider
 * (an MCP credential is the same secret whichever model is driving the MCP
 * client).
 */
export interface PlatformInlineSecrets {
  readonly apiKeys?: Partial<Record<ProviderName, string>>;
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

function parseEnvironment(input: unknown): PlatformEnvironment | undefined {
  if (input === undefined) {
    return undefined;
  }
  const value = requireRecord(input, "submission.environment");
  const allowed = defineAllowedKeys<PlatformEnvironmentInput>()("networking", "packages", "envVars");
  assertAllowedKeys(
    value,
    allowed,
    (key) => new Error(`submission.environment.${key} is not an allowed field; permitted: networking, packages, envVars`)
  );
  const networking = parseNetworking(value.networking);
  const packages = parsePackages(value.packages);
  const envVars = parseEnvVars(value.envVars);
  if (!networking && !packages && !envVars) {
    return undefined;
  }
  return {
    ...(networking ? { networking } : {}),
    ...(packages ? { packages } : {}),
    ...(envVars ? { envVars } : {})
  };
}

/**
 * Validate a customer-supplied `environment.envVars` map. Returns a
 * frozen copy with keys in insertion order, or `undefined` when the
 * input is absent / an empty object (treated as not supplied so the
 * hosted API can omit the field from the parsed snapshot).
 *
 * Rules:
 *   - Must be a JSON object whose values are all strings.
 *   - Keys match `[A-Z_][A-Z0-9_]*` (POSIX-shell portable, uppercase
 *     only — keeps RUNTIME.env readable, matches platform convention).
 *   - Keys MUST NOT start with the reserved `AEX_` prefix; that
 *     prefix is owned by platform-set runtime keys and a collision
 *     would silently mask `__AEX_CLI__` etc. substitution
 *     targets.
 *   - Bounded: max ENV_VARS_MAX_ENTRIES entries, max
 *     ENV_VARS_MAX_VALUE_BYTES per value, max ENV_VARS_MAX_TOTAL_BYTES
 *     overall. The caps stop a sessionaway customer from making the
 *     mounted RUNTIME files unbounded.
 *   - Values are arbitrary UTF-8 strings, EXCEPT NUL bytes are
 *     rejected (NUL terminates C-strings and breaks env-var
 *     transport even inside the container).
 */
function parseEnvVars(input: unknown): Readonly<Record<string, string>> | undefined {
  if (input === undefined) {
    return undefined;
  }
  const value = requireRecord(input, "submission.environment.envVars");
  const keys = Object.keys(value);
  if (keys.length === 0) {
    return undefined;
  }
  if (keys.length > ENV_VARS_MAX_ENTRIES) {
    throw new Error(
      `submission.environment.envVars has ${keys.length} entries; maximum is ${ENV_VARS_MAX_ENTRIES}`
    );
  }
  const out: Record<string, string> = {};
  let totalBytes = 0;
  for (const key of keys) {
    if (!ENV_VAR_KEY_PATTERN.test(key)) {
      throw new Error(
        `submission.environment.envVars.${key} key must match /^[A-Z_][A-Z0-9_]*$/`
      );
    }
    if (key.startsWith(AEX_RESERVED_ENV_PREFIX)) {
      throw new Error(
        `submission.environment.envVars.${key} uses reserved prefix "${AEX_RESERVED_ENV_PREFIX}" (set by the aex runtime)`
      );
    }
    const raw = value[key];
    if (typeof raw !== "string") {
      throw new Error(`submission.environment.envVars.${key} must be a string`);
    }
    if (raw.includes("\0")) {
      throw new Error(`submission.environment.envVars.${key} must not contain NUL bytes`);
    }
    const valueBytes = Buffer.byteLength(raw, "utf8");
    if (valueBytes > ENV_VARS_MAX_VALUE_BYTES) {
      throw new Error(
        `submission.environment.envVars.${key} value is ${valueBytes} bytes; maximum is ${ENV_VARS_MAX_VALUE_BYTES}`
      );
    }
    totalBytes += Buffer.byteLength(key, "utf8") + valueBytes;
    if (totalBytes > ENV_VARS_MAX_TOTAL_BYTES) {
      throw new Error(
        `submission.environment.envVars total byte size exceeds maximum ${ENV_VARS_MAX_TOTAL_BYTES}`
      );
    }
    out[key] = raw;
  }
  return Object.freeze(out);
}

function parseNetworking(input: unknown): PlatformNetworking | undefined {
  if (input === undefined) {
    return undefined;
  }
  const value = requireRecord(input, "submission.environment.networking");
  const allowed = defineAllowedKeys<PlatformNetworking>()("mode", "allowedHosts");
  assertAllowedKeys(
    value,
    allowed,
    (key) => new Error(`submission.environment.networking.${key} is not an allowed field; permitted: mode, allowedHosts`)
  );
  const mode = optionalEnum(value.mode, "submission.environment.networking.mode", ["limited", "open"]);
  if (!mode) {
    throw new Error("submission.environment.networking.mode is required when networking is provided");
  }
  const allowedHosts = parseAllowedHosts(value.allowedHosts);
  return allowedHosts ? { mode, allowedHosts } : { mode };
}

function parseAllowedHosts(input: unknown): readonly string[] | undefined {
  if (input === undefined) {
    return undefined;
  }
  if (!Array.isArray(input)) {
    throw new Error("submission.environment.networking.allowedHosts must be an array of strings");
  }
  const seen = new Set<string>();
  return input.map((entry, index) => {
    if (typeof entry !== "string" || entry.length === 0) {
      throw new Error(`submission.environment.networking.allowedHosts[${index}] must be a non-empty string`);
    }
    const lower = entry.toLowerCase();
    if (seen.has(lower)) {
      throw new Error(`submission.environment.networking.allowedHosts duplicate entry: ${entry}`);
    }
    seen.add(lower);
    return lower;
  });
}

function parsePackages(input: unknown): readonly PlatformPackage[] | undefined {
  if (input === undefined) {
    return undefined;
  }
  if (!Array.isArray(input)) {
    throw new Error("submission.environment.packages must be an array");
  }
  return input.map((entry, index) => {
    const value = requireRecord(entry, `submission.environment.packages[${index}]`);
    const allowed = defineAllowedKeys<PlatformPackageInput>()("name", "version");
    assertAllowedKeys(
      value,
      allowed,
      (key) => new Error(`submission.environment.packages[${index}].${key} is not an allowed field; permitted: name, version`)
    );
    const rawName = requireString(value.name, `submission.environment.packages[${index}].name`);
    const version = optionalString(value.version, `submission.environment.packages[${index}].version`);
    // The ecosystem is encoded as a `name` prefix `"<eco>:<pkg>"`; an
    // unprefixed name defaults to `apt`. A colon-delimited prefix that is
    // NOT a known ecosystem is rejected (rather than silently folded into
    // the package name) so a typo'd manager fails closed.
    let ecosystem: PlatformPackageEcosystem = "apt";
    let name = rawName;
    const colon = rawName.indexOf(":");
    if (colon > 0) {
      const prefix = rawName.slice(0, colon);
      if (!(PLATFORM_PACKAGE_ECOSYSTEMS as readonly string[]).includes(prefix)) {
        throw new Error(
          `submission.environment.packages[${index}].name has unknown ecosystem prefix "${prefix}:"; permitted: ${PLATFORM_PACKAGE_ECOSYSTEMS.join(", ")}`
        );
      }
      ecosystem = prefix as PlatformPackageEcosystem;
      name = rawName.slice(colon + 1);
    }
    if (name.length === 0) {
      throw new Error(
        `submission.environment.packages[${index}].name resolves to an empty package after stripping the "${ecosystem}:" ecosystem prefix`
      );
    }
    const parsed = version ? { name, version, ecosystem } : { name, ecosystem };
    assertPlatformPackage(parsed, `submission.environment.packages[${index}]`);
    return parsed;
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
  // Absent/null secrets collapse to an empty bundle; the credential-policy gate
  // (enforceCredentialSecretPolicy) decides whether that is admissible for the
  // session's mode (a session inheriting keys server-side may legitimately omit them).
  if (input === undefined || input === null) return {};
  const value = requireRecord(input, "secrets");
  const allowedTopLevel = defineAllowedKeys<PlatformInlineSecrets>()("apiKeys", "mcpServers", "envSecrets");
  assertAllowedKeys(
    value,
    allowedTopLevel,
    (key, orderedKeys) => key.startsWith("__aex_")
      ? new Error(`secrets.${key} uses the platform-internal __aex_ namespace and may not be set by callers`)
      : new UnknownFieldError("secrets", key, orderedKeys)
  );
  const apiKeys = parseApiKeys(value.apiKeys);
  const mcpServers = parseMcpServerSecrets(value.mcpServers);
  const envSecrets = parseEnvSecrets(value.envSecrets);

  return {
    ...(apiKeys ? { apiKeys } : {}),
    ...(mcpServers ? { mcpServers } : {}),
    ...(envSecrets ? { envSecrets } : {})
  };
  });
}

/**
 * Parse the per-provider BYOK key map. Each key must name a known
 * {@link ProviderName}; each value must be a non-empty string. Returns
 * `undefined` for an absent or empty map so the spread above stays clean.
 */
function parseApiKeys(input: unknown): Partial<Record<ProviderName, string>> | undefined {
  if (input === undefined || input === null) return undefined;
  const value = requireRecord(input, "secrets.apiKeys");
  const out: Partial<Record<ProviderName, string>> = {};
  for (const [provider, key] of Object.entries(value)) {
    if (!(PROVIDERS as readonly string[]).includes(provider)) {
      throw new Error(
        `secrets.apiKeys["${provider}"] is not a known provider; permitted: ${PROVIDERS.join(", ")}`
      );
    }
    if (typeof key !== "string" || key.length === 0) {
      throw new Error(`secrets.apiKeys["${provider}"] must be a non-empty string`);
    }
    out[provider as ProviderName] = key;
  }
  return Object.keys(out).length > 0 ? out : undefined;
}

function parseEnvSecrets(input: unknown): Readonly<Record<string, string>> | undefined {
  if (input === undefined || input === null) return undefined;
  const value = requireRecord(input, "secrets.envSecrets");
  const out: Record<string, string> = {};
  for (const [envName, entry] of Object.entries(value)) {
    if (!SECRET_ENV_NAME_PATTERN.test(envName)) {
      throw new Error(
        `secrets.envSecrets key "${envName}" must be a valid env var name matching ${SECRET_ENV_NAME_PATTERN.source}`
      );
    }
    if (typeof entry !== "string" || entry.length === 0) {
      throw new Error(`secrets.envSecrets.${envName} must be a non-empty string`);
    }
    out[envName] = entry;
  }
  return Object.keys(out).length > 0 ? out : undefined;
}

function parseMcpServerSecrets(input: unknown): readonly PlatformMcpServerSecret[] | undefined {
  if (input === undefined) {
    return undefined;
  }
  if (!Array.isArray(input)) {
    throw new Error("secrets.mcpServers must be an array");
  }
  const seen = new Set<string>();
  return input.map((entry, index) => {
    const parsed = parseMcpServerSecret(entry, `secrets.mcpServers[${index}]`);
    if (seen.has(parsed.name)) {
      throw new Error(`secrets.mcpServers duplicate name: ${parsed.name}`);
    }
    seen.add(parsed.name);
    return parsed;
  });
}

function parseMcpServerSecret(input: unknown, path: string): PlatformMcpServerSecret {
  const value = requireRecord(input, path);
  const allowed = defineAllowedKeys<PlatformMcpServerSecret>()("name", "url", "headers");
  assertAllowedKeys(value, allowed, (key) => new Error(`${path}.${key} is not an allowed field; permitted: name, url, headers`));
  const name = requireString(value.name, `${path}.name`);
  const url = requireString(value.url, `${path}.url`);
  const headers = optionalStringRecord(value.headers, `${path}.headers`);
  return headers ? { name, url, headers } : { name, url };
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
   * arrive, THEN a final coalesced block. Streaming is CAPABILITY-GATED: it is
   * only honored for a streamable provider (see {@link STREAMABLE_SHAPES}) and a
   * `stream` mode on a non-streamable provider is rejected at parse
   * ({@link assertStreamableOutputMode}) — no silent downgrade.
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
  /**
   * Provider selector. Always populated after parsing — absent on the
   * wire means {@link DEFAULT_PROVIDER}. All providers are dispatched
   * through the managed runtime.
   */
  readonly provider: ProviderName;
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
  "workspaceId" | "provider" | "timeoutMs"
> & {
  readonly workspaceId?: string;
  readonly provider?: ProviderName;
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
  const value = requireRecord(input, "submission");
  const allowedTopLevelFields = defineAllowedKeys<PlatformSessionSubmissionInput>()(
    "workspaceId",
    "idempotencyKey",
    "provider",
    "submission",
    "runtimeSize",
    "runtimeKind",
    "timeout",
    "webhook",
    "limits",
    "machine",
    SECRETS_KEY
  );
  assertAllowedKeys(
    value,
    allowedTopLevelFields,
    (key, orderedKeys) => new Error(`submission.${key} is not an allowed field; permitted: ${orderedKeys.join(", ")}`)
  );
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
  const provider = parseProviderName(value.provider);
  const runtimeSize = parseRuntimeSize(value.runtimeSize);
  const runtimeKind = parseRuntimeKind(value.runtimeKind);
  const timeoutMs = parseSessionTimeout(value.timeout);
  const webhook = parseSessionWebhook(value.webhook);
  const limits = parseSessionLimits(value.limits);
  const machine = parseSessionMachine(value.machine);
  const secrets = parseInlineSecrets(value.secrets);
  enforceCredentialSecretPolicy(secrets, provider);

  const submission = parseSubmission(value.submission);
  assertModelNameMatchesProvider(provider, submission.model);
  // Fail-closed streaming: `outputMode:'stream'` on a non-streamable provider is
  // a hard reject at parse time (no silent downgrade).
  assertStreamableOutputMode(submission.outputMode, provider);

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
    provider,
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
  const value = requireRecord(input, "webhook");
  const allowed = defineAllowedKeys<SessionWebhookSpec>()("url");
  assertAllowedKeys(value, allowed, (key, orderedKeys) =>
    new Error(`webhook.${key} is not an allowed field; permitted: ${orderedKeys.join(", ")}`)
  );
  const url = requireString(value.url, "webhook.url");
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    throw new Error(`webhook.url must be a valid absolute URL (got ${JSON.stringify(url)})`);
  }
  if (parsed.protocol !== "https:") {
    throw new Error(`webhook.url must use https (got ${parsed.protocol.replace(/:$/, "")})`);
  }
  if (parsed.username !== "" || parsed.password !== "") {
    throw new Error("webhook.url must not contain userinfo (user:pass@host)");
  }
  return { url };
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
  const value = requireRecord(input, "limits");
  const allowed = defineAllowedKeys<SessionLimits>()(
    "maxConcurrentChildSessions",
    "maxSubagentDepth",
    "maxSpendUsd",
    "maxTurns",
    "maxStepsPerTurn"
  );
  assertAllowedKeys(value, allowed, (key, orderedKeys) =>
    new Error(`limits.${key} is not an allowed field; permitted: ${orderedKeys.join(", ")}`)
  );
  const maxConcurrentChildSessions = optionalPositiveInt(
    value.maxConcurrentChildSessions,
    "limits.maxConcurrentChildSessions"
  );
  const maxSubagentDepth = optionalPositiveInt(value.maxSubagentDepth, "limits.maxSubagentDepth");
  // maxSpendUsd is a USD amount (may be fractional, e.g. $2.50) so it is a positive
  // NUMBER, not a positive int. Clamping to the workspace/platform ceiling is the
  // resolver's job; here we only enforce shape + positivity.
  const maxSpendUsd = optionalPositiveNumber(value.maxSpendUsd, "limits.maxSpendUsd");
  // maxTurns is an ITERATION count — a positive safe integer. Clamp to the ceiling
  // is the resolver's job; here we enforce shape + positivity only.
  const maxTurns = optionalPositiveInt(value.maxTurns, "limits.maxTurns");
  // maxStepsPerTurn is a per-turn STEP count — a positive safe integer (the
  // runaway-loop backstop one level below maxTurns). Same shape/positivity gate;
  // the resolver clamps to the ceiling.
  const maxStepsPerTurn = optionalPositiveInt(value.maxStepsPerTurn, "limits.maxStepsPerTurn");
  // Collapse an all-absent override (e.g. `limits: {}`) to `undefined` so it never
  // lands an empty object on the request — matches sibling parsers (parseSessionWebhook,
  // parseEnvironment). The resolver supplies platform defaults for absent fields.
  if (
    maxConcurrentChildSessions === undefined &&
    maxSubagentDepth === undefined &&
    maxSpendUsd === undefined &&
    maxTurns === undefined &&
    maxStepsPerTurn === undefined
  ) {
    return undefined;
  }
  return {
    ...(maxConcurrentChildSessions !== undefined ? { maxConcurrentChildSessions } : {}),
    ...(maxSubagentDepth !== undefined ? { maxSubagentDepth } : {}),
    ...(maxSpendUsd !== undefined ? { maxSpendUsd } : {}),
    ...(maxTurns !== undefined ? { maxTurns } : {}),
    ...(maxStepsPerTurn !== undefined ? { maxStepsPerTurn } : {})
  };
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
  const value = requireRecord(input, "machine");
  const allowed = defineAllowedKeys<SessionMachine>()("spot");
  assertAllowedKeys(value, allowed, (key, orderedKeys) =>
    new Error(`machine.${key} is not an allowed field; permitted: ${orderedKeys.join(", ")}`)
  );
  if (value.spot !== undefined && typeof value.spot !== "boolean") {
    throw new Error("machine.spot must be a boolean");
  }
  if (value.spot === undefined) {
    return undefined;
  }
  return { spot: value.spot };
  });
}

export function parseProviderName(input: unknown): ProviderName {
  return withContractParseError("parseProviderName", () => {
  if (input === undefined) {
    return DEFAULT_PROVIDER;
  }
  if (typeof input !== "string" || !(PROVIDERS as readonly string[]).includes(input)) {
    throw new Error(
      `provider must be one of: ${PROVIDERS.join(", ")} (got ${JSON.stringify(input)})`
    );
  }
  return input as ProviderName;
  });
}

/**
 * Cross-check the supplied secrets bundle against the credential mode. BYOK
 * requires `secrets.apiKeys[provider]` (the key for the session's own `provider`).
 * Additional provider keys are optional (validated for shape only) so the session
 * can supply keys for the other providers its subagents may use. MCP / proxy
 * endpoint auth carry across providers and are not checked here.
 *
 * A CHILD session (`inheritsFromParent`) is exempt from the own-key requirement: it
 * inherits its provider keys server-side from the parent's vaulted bundle, so
 * it need not carry any of its own. The server still verifies, at admission,
 * that the parent actually holds a key for the child's provider.
 */
export function enforceCredentialSecretPolicy(
  secrets: PlatformInlineSecrets,
  provider: ProviderName,
  opts?: { readonly inheritsFromParent?: boolean }
): void {
  if (opts?.inheritsFromParent) return;
  if (!secrets.apiKeys?.[provider]) {
    throw new Error(
      `secrets.apiKeys["${provider}"] is required`
    );
  }
}

export function parseSubmission(input: unknown): PlatformSubmission {
  return withContractParseError("parseSubmission", () => {
  const value = requireRecord(input, "submission.submission");
  const allowed = defineAllowedKeys<PlatformSubmission>()(
    "model",
    "system",
    "prompt",
    "assets",
    "mcpServers",
    "secretEnv",
    "environment",
    "securityProfile",
    "metadata",
    "fileCapture",
    "builtinTools",
    "outputMode",
    "responseFormat",
    "approvalGate",
    "platform"
  );
  assertAllowedKeys(value, allowed, (key, orderedKeys) =>
    new Error(`submission.${key} is not an allowed field; permitted: ${orderedKeys.join(", ")}`)
  );
  const model = parseModelName(value.model, "submission.model");
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
  const value = requireRecord(input, "submission.assets");
  const allowed = defineAllowedKeys<SubmissionAssets>()("files", "skills", "tools", "instructions");
  assertAllowedKeys(
    value,
    allowed,
    (key) => new Error(`submission.assets.${key} is not allowed; permitted: files, skills, tools, instructions`)
  );
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
    defineAllowedKeys<WorkspaceFileRef>()("kind", "resourceId", "version", "assetId", "contentHash", "name", "mountPath"),
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
    defineAllowedKeys<WorkspaceSkillRef>()("kind", "resourceId", "version", "assetId", "contentHash", "name", "description"),
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
    defineAllowedKeys<WorkspaceToolRef>()(
      "kind",
      "resourceId",
      "version",
      "assetId",
      "contentHash",
      "name",
      "description",
      "input_schema",
      "entry"
    ),
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
    defineAllowedKeys<WorkspaceInstructionRef>()("kind", "resourceId", "version", "assetId", "contentHash", "name"),
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
  allowedFields: readonly string[],
  project: (raw: Record<string, unknown>, base: PinnedResourceBase, path: string) => T
): readonly T[] {
  if (input === undefined) return [];
  if (!Array.isArray(input)) throw new Error(`submission.assets.${field} must be an array`);
  const seen = new Set<string>();
  return input.map((item, index) => {
    const path = `submission.assets.${field}[${index}]`;
    const raw = requireRecord(item, path);
    assertAllowedKeys(raw, allowedFields, (key) => new Error(`${path}.${key} is not allowed`));
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
  const value = requireRecord(input, "submission.platform");
  const allowed = defineAllowedKeys<PlatformInjectionConfig>()("systemPrompt");
  assertAllowedKeys(
    value,
    allowed,
    (key) => new Error(`submission.platform.${key} is not an allowed field; permitted: systemPrompt`)
  );
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
// Streaming capability model — WS9. `outputMode:'stream'` is capability-gated.
// ---------------------------------------------------------------------------

/**
 * The provider wire-SHAPES that have a real per-token streaming producer. This
 * const is the contracts-side SSoT, pinned EQUAL to the platform's shape SSoT by
 * a cross-repo parity test so streaming can never be promised for a shape
 * nothing feeds.
 */
export const STREAMABLE_SHAPES = ["anthropic", "openai_chat"] as const;
export type StreamableShape = (typeof STREAMABLE_SHAPES)[number];

/**
 * Each provider's wire shape, or `null` when it has no streaming producer wired
 * yet. `stream` output is only honored for a provider whose shape is streamable.
 */
const PROVIDER_STREAM_SHAPE = {
  anthropic: "anthropic",
  deepseek: "openai_chat",
  openai: "openai_chat",
  gemini: null,
  mistral: "openai_chat",
  openrouter: "openai_chat",
  doubao: "openai_chat"
} as const satisfies Readonly<Record<ProviderName, StreamableShape | null>>;

/** True when a provider has a streaming producer wired (a {@link STREAMABLE_SHAPES} shape). */
export function isStreamableProvider(provider: ProviderName): boolean {
  return PROVIDER_STREAM_SHAPE[provider] !== null;
}

function streamableProviders(): readonly ProviderName[] {
  return (Object.keys(PROVIDER_STREAM_SHAPE) as ProviderName[]).filter(isStreamableProvider);
}

/**
 * Fail-closed streaming gate: `outputMode:'stream'` on a NON-streamable provider
 * throws (a HARD reject — no silent downgrade to buffered). Called by
 * {@link parseSessionSubmissionRequest} once mode + provider are both known.
 */
export function assertStreamableOutputMode(outputMode: OutputMode | undefined, provider: ProviderName): void {
  if (outputMode === "stream" && !isStreamableProvider(provider)) {
    throw new Error(
      `submission.outputMode 'stream' is not supported for provider ${provider}; ` +
        `streaming is available for: ${streamableProviders().join(", ")}`
    );
  }
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
  if (kind === "text") {
    const allowed = defineAllowedKeys<Extract<ResponseFormat, { readonly kind: "text" }>>()("kind");
    assertAllowedKeys(
      value,
      allowed,
      (key) => new Error(`submission.responseFormat.${key} is not allowed when kind is 'text'`)
    );
    return { kind: "text" };
  }
  const allowed = defineAllowedKeys<Extract<ResponseFormat, { readonly kind: "json_schema" }>>()(
    "kind",
    "schema",
    "strict",
    "name"
  );
  assertAllowedKeys(
    value,
    allowed,
    (key, orderedKeys) => new Error(`submission.responseFormat.${key} is not an allowed field; permitted: ${orderedKeys.join(", ")}`)
  );
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
  const value = requireRecord(input, "submission.approvalGate");
  const allowed = defineAllowedKeys<ApprovalGate>()("tools");
  assertAllowedKeys(
    value,
    allowed,
    (key) => new Error(`submission.approvalGate.${key} is not an allowed field; permitted: tools`)
  );
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
  const value = requireRecord(input, "submission.fileCapture");
  const allowed = defineAllowedKeys<PlatformFileCaptureConfig>()(
    "allowedDirs",
    "deniedDirs",
    "captureTimeoutMs",
    "maxFileBytes",
    "maxTotalBytes",
    "maxFiles"
  );
  assertAllowedKeys(value, allowed, (key, orderedKeys) =>
    new Error(`submission.fileCapture.${key} is not an allowed field; permitted: ${orderedKeys.join(", ")}`)
  );
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

