import {
  authShapeHeaderName,
  authShapeQueryName,
  PROXY_ALLOWED_METHODS,
  PROXY_ENDPOINT_DEFAULTS,
  PROXY_RESPONSE_MODES,
  type ProxyAuthShape,
  type ProxyAuthType,
  type ProxyMethod,
  type ProxyResponseMode
} from "./proxy-protocol.js";

// Re-exported from the protocol module (its canonical home, alongside the
// index-file shape the builder fills). Kept on the submission surface so
// existing `@antpath/contracts` consumers of `PROXY_ENDPOINT_DEFAULTS` are
// unaffected by the move.
export { PROXY_ENDPOINT_DEFAULTS };
import { parseMcpServerRef, parseR2RefFields, parseSkillRef } from "./run-config.js";
import type {
  AgentsMdRef,
  FileRef,
  McpServerRef,
  SkillRef
} from "./run-config.js";
import { parseMachineSize, parseRunTimeout, type MachineSize } from "./machine-sizes.js";
import {
  NATIVE_RUNTIME_PROVIDERS,
  PROVIDER_CAPABILITY,
  providerHasNativeAgent
} from "./provider-capability.js";
export {
  NATIVE_RUNTIME_PROVIDERS,
  PROVIDER_CAPABILITY,
  providerHasNativeAgent
} from "./provider-capability.js";
export type {
  NativeAgentCapability,
  NativeExecutorId,
  NativeRuntimeProvider,
  ProviderCapability
} from "./provider-capability.js";
import {
  parseRuntimeSecurityProfile,
  type RuntimeSecurityProfileName
} from "./runtime-security-profile.js";
import {
  assertManagedKeyAdmissionAllowed,
  parseCredentialMode,
  type CredentialMode,
  type ManagedKeyPolicyV1
} from "./managed-key.js";

export type JsonPrimitive = string | number | boolean | null;
export type JsonValue = JsonPrimitive | JsonValue[] | { readonly [key: string]: JsonValue };

/**
 * Networking + runtime-package snapshot carried inside a flat submission
 * so the worker can deep-clone and mutate it per run (e.g. injecting the
 * proxy hostname into `allowed_hosts`) without sharing state across
 * concurrent runs.
 *
 * `envVars` is the customer-controlled key/value bag delivered into the
 * container via the mounted `RUNTIME.env` / `RUNTIME.json` files (the
 * Anthropic session API does NOT expose a process env-vars knob today
 * — verified by reading the `/v1/environments` config shape). The same
 * keys become `__KEY__` substitution targets in agent-facing markdown
 * inside skill / agentsmd / file bundles. Antpath-set runtime keys use
 * the reserved `ANTPATH_*` prefix; customer keys MUST NOT collide with
 * that prefix.
 */
export interface PlatformEnvironment {
  readonly networking?: PlatformNetworking;
  readonly packages?: readonly PlatformPackage[];
  readonly envVars?: Readonly<Record<string, string>>;
}

/**
 * Reserved prefix for antpath-set runtime env vars (`ANTPATH_OUTPUTS`,
 * `ANTPATH_CLI`, …). Customer `environment.envVars` keys carrying this
 * prefix are rejected at submission parse time so platform-set values
 * cannot be silently overwritten.
 */
export const ANTPATH_RESERVED_ENV_PREFIX = "ANTPATH_";

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
  /** Lowercase host names. The worker always appends the proxy host. */
  readonly allowedHosts?: readonly string[];
}

/**
 * Package-manager ecosystems the Anthropic Managed Agents
 * `POST /v1/environments` `config.packages` map accepts (one list per
 * manager). The customer encodes the target manager as a `name` prefix
 * `"<eco>:<pkg>"` (e.g. "pip:pandas", "npm:express", "apt:ffmpeg"); an
 * UNPREFIXED name defaults to `apt`. After parsing, `PlatformPackage.name`
 * is the bare package and `PlatformPackage.ecosystem` is the resolved
 * manager.
 */
export const PLATFORM_PACKAGE_ECOSYSTEMS = ["apt", "cargo", "gem", "go", "npm", "pip"] as const;
export type PlatformPackageEcosystem = (typeof PLATFORM_PACKAGE_ECOSYSTEMS)[number];

export interface PlatformPackage {
  readonly name: string;
  readonly version?: string;
  readonly ecosystem: PlatformPackageEcosystem;
}

/**
 * Render a parsed {@link PlatformPackage} as the version-embedded install
 * string the Anthropic `config.packages` map expects for its ecosystem.
 * The join differs per manager (verified against the live beta API):
 *   - pip  → `name==version`
 *   - npm / cargo / go → `name@version`
 *   - gem  → `name:version`
 *   - apt  → `name=version`
 * With no `version`, just the bare `name`. Pure; shared by the native
 * materializer (grouped map) and the Goose runner (per-package install arg).
 */
export function nativePackageString(pkg: PlatformPackage): string {
  if (pkg.version === undefined) {
    return pkg.name;
  }
  switch (pkg.ecosystem) {
    case "pip":
      return `${pkg.name}==${pkg.version}`;
    case "npm":
    case "cargo":
    case "go":
      return `${pkg.name}@${pkg.version}`;
    case "gem":
      return `${pkg.name}:${pkg.version}`;
    case "apt":
      return `${pkg.name}=${pkg.version}`;
  }
}

export type PlatformSessionCleanup = "retain" | "delete";

export interface PlatformCleanupPolicy {
  readonly session?: PlatformSessionCleanup;
}

export interface PlatformAnthropicSecrets {
  readonly apiKey: string;
  readonly baseUrl?: string;
}

export interface PlatformDeepseekSecrets {
  readonly apiKey: string;
  readonly baseUrl?: string;
}

export interface PlatformOpenAISecrets {
  readonly apiKey: string;
  readonly baseUrl?: string;
}

export interface PlatformGeminiSecrets {
  readonly apiKey: string;
  readonly baseUrl?: string;
}

export interface PlatformMistralSecrets {
  readonly apiKey: string;
  readonly baseUrl?: string;
}

/**
 * Run-time provider selector. Antpath exposes one customer interface
 * for every provider; the runtime that backs each provider is decided
 * by {@link selectRuntime}.
 *
 *   - `anthropic` — defaults to the Anthropic Native runtime (Anthropic
 *                   Managed Agents API). Customers can opt into the
 *                   Goose Managed runtime via `runtime: "managed"`.
 *   - `deepseek` | `openai` | `gemini` | `mistral` — only the Goose
 *                   Managed runtime is supported (the upstream model is
 *                   reached via the hosted BYOK provider-proxy).
 *
 * Runtime routing is derived by {@link selectRuntime}.
 */
export const RUN_PROVIDERS = [
  "anthropic",
  "deepseek",
  "openai",
  "gemini",
  "mistral"
] as const;
export type RunProvider = (typeof RUN_PROVIDERS)[number];
export const DEFAULT_RUN_PROVIDER: RunProvider = "anthropic";

/**
 * Customer-facing runtime selector. Optional on the wire — absent means
 * "let the dispatcher route based on provider" ({@link selectRuntime}).
 *
 *   - `native`  — Anthropic Native runtime. Only valid for
 *                 `provider: "anthropic"`. Routes through Anthropic's
 *                 Managed Agents API.
 *   - `managed` — Goose Managed runtime. The only option for
 *                 non-Anthropic providers; also available as an opt-out
 *                 for `provider: "anthropic"` when the customer wants
 *                 cross-provider parity.
 *
 * Stored verbatim in `runs.runtime`; runtime dispatch remains explicit and
 * fail-closed.
 */
export const RUNTIME_KINDS = ["native", "managed"] as const;
export type RuntimeKind = (typeof RUNTIME_KINDS)[number];

/** Outcome of the centralized runtime-support check. */
export interface RuntimeSupportCheck {
  readonly ok: boolean;
  readonly code?: "runtime_native_unsupported";
  readonly message?: string;
}

/**
 * Centralized runtime-support validator (single source of truth for the
 * native-first strategy). The provider-capability registry declares which
 * providers have a native agent runtime; this is the one place that checks a
 * requested runtime against it. Pure + result-typed so BOTH planes enforce
 * the same rule:
 *
 *   - The SDK calls it client-side and fails EARLY (throws before the HTTP
 *     request) when the caller explicitly asked for `runtime: "native"` on a
 *     provider with no native runtime.
 *   - The server (the submission parser + {@link selectRuntime}) calls it and
 *     raises a `RuntimeValidationError`.
 *
 * An ABSENT runtime is always ok: the dispatcher auto-routes native-first and
 * falls back to managed. Feature-level native gaps are validated separately,
 * server-side, against the resolved submission.
 */
export function checkRuntimeSupported(
  provider: RunProvider,
  runtime: RuntimeKind | undefined
): RuntimeSupportCheck {
  if (runtime === "native" && !isNativeRuntimeProvider(provider)) {
    return {
      ok: false,
      code: "runtime_native_unsupported",
      message: `runtime: "native" is only supported for provider: ${formatProviderList(
        NATIVE_RUNTIME_PROVIDERS
      )} (got provider: "${provider}")`
    };
  }
  return { ok: true };
}

export interface PlatformMcpServerSecret {
  readonly name: string;
  readonly url: string;
  readonly headers?: Record<string, string>;
}

/**
 * Per-run auth value for a declared proxy endpoint. The `name` must
 * match a `proxyEndpoints[i].name` in the same submission, and `value`'s
 * shape must match that endpoint's `authShape.type`. The cross-validation
 * lives in `parseRunSubmissionRequest`.
 */
export interface PlatformProxyEndpointAuth {
  readonly name: string;
  readonly value: PlatformProxyAuthValue;
}

export type PlatformProxyAuthValue =
  | { readonly type: "bearer"; readonly token: string }
  | { readonly type: "basic"; readonly username: string; readonly password: string }
  | { readonly type: "header"; readonly value: string }
  | { readonly type: "query"; readonly value: string };

/**
 * Per-run inline secrets bundle. Exactly one of `anthropic` | `deepseek`
 * | `openai` | `gemini` | `mistral` is required, matching the run's
 * `provider`; the cross-provider coupling is enforced in
 * `parseRunSubmissionRequest` so the wire shape stays simple and
 * individual provider keys remain optional in the type system.
 * `mcpServers` and `proxyEndpointAuth` are cross-provider (an MCP
 * credential is the same secret whether Anthropic or another model is
 * driving the MCP client).
 */
export interface PlatformInlineSecrets {
  readonly anthropic?: PlatformAnthropicSecrets;
  readonly deepseek?: PlatformDeepseekSecrets;
  readonly openai?: PlatformOpenAISecrets;
  readonly gemini?: PlatformGeminiSecrets;
  readonly mistral?: PlatformMistralSecrets;
  readonly mcpServers?: readonly PlatformMcpServerSecret[];
  readonly proxyEndpointAuth?: readonly PlatformProxyEndpointAuth[];
}

/**
 * Per-run named HTTP proxy endpoint. The `authShape` describes how the
 * upstream expects auth; the actual value is supplied separately via
 * `secrets.proxyEndpointAuth`. The auth value never enters the
 * container — the BFF proxy injects it on outbound calls.
 *
 * Caps and allow-lists below are intentionally pessimistic by default
 * so a misconfigured endpoint can't accidentally permit a wide attack
 * surface; raise per endpoint if needed.
 */
export interface PlatformProxyEndpoint {
  readonly name: string;
  readonly baseUrl: string;
  readonly authShape: ProxyAuthShape;
  readonly allowMethods: readonly ProxyMethod[];
  readonly allowPathPrefixes: readonly string[];
  readonly allowHeaders?: readonly string[];
  readonly responseMode?: ProxyResponseMode;
  readonly maxRequestBytes?: number;
  readonly maxResponseBytes?: number;
  readonly timeoutMs?: number;
  readonly perCallBudget?: number;
  readonly responseByteBudget?: number;
}

const SECRETS_KEY = "secrets";

const PROXY_ENDPOINT_NAME_PATTERN = /^[a-z][a-z0-9_-]{0,62}$/;
const RESERVED_PROXY_ENDPOINT_NAMES = new Set(["proxy", "antpath", "internal", "admin"]);

/**
 * Headers the proxy never lets through, regardless of policy. Lowercase.
 * Anything that could re-introduce credentials, cookies, or routing
 * primitives. Kept in lockstep with the proxy route's reject list.
 */
const PROXY_DENY_HEADER_LIST = new Set([
  "authorization",
  "cookie",
  "set-cookie",
  "proxy-authorization",
  "host",
  "content-length",
  "transfer-encoding",
  "connection",
  "upgrade",
  "expect",
  "x-forwarded-for",
  "x-forwarded-host",
  "x-forwarded-proto",
  "x-real-ip"
]);

const deniedSecretFields = new Set([
  "providerApiKey",
  "anthropicApiKey",
  "apiKey",
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
  const allowed = new Set(["networking", "packages", "envVars"]);
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) {
      throw new Error(
        `submission.environment.${key} is not an allowed field; permitted: networking, packages, envVars`
      );
    }
  }
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
 * worker can omit the field from the parsed snapshot).
 *
 * Rules:
 *   - Must be a JSON object whose values are all strings.
 *   - Keys match `[A-Z_][A-Z0-9_]*` (POSIX-shell portable, uppercase
 *     only — keeps RUNTIME.env readable, matches platform convention).
 *   - Keys MUST NOT start with the reserved `ANTPATH_` prefix; that
 *     prefix is owned by platform-set runtime keys and a collision
 *     would silently mask `__ANTPATH_OUTPUTS__` etc. substitution
 *     targets.
 *   - Bounded: max ENV_VARS_MAX_ENTRIES entries, max
 *     ENV_VARS_MAX_VALUE_BYTES per value, max ENV_VARS_MAX_TOTAL_BYTES
 *     overall. The caps stop a runaway customer from making the
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
    if (key.startsWith(ANTPATH_RESERVED_ENV_PREFIX)) {
      throw new Error(
        `submission.environment.envVars.${key} uses reserved prefix "${ANTPATH_RESERVED_ENV_PREFIX}" (set by antpath runtime)`
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
  const allowed = new Set(["mode", "allowedHosts"]);
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) {
      throw new Error(`submission.environment.networking.${key} is not an allowed field; permitted: mode, allowedHosts`);
    }
  }
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
    const allowed = new Set(["name", "version"]);
    for (const key of Object.keys(value)) {
      if (!allowed.has(key)) {
        throw new Error(`submission.environment.packages[${index}].${key} is not an allowed field; permitted: name, version`);
      }
    }
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
    return version ? { name, version, ecosystem } : { name, ecosystem };
  });
}

function parseProxyEndpoints(input: unknown): readonly PlatformProxyEndpoint[] | undefined {
  if (input === undefined) {
    return undefined;
  }
  if (!Array.isArray(input)) {
    throw new Error("proxyEndpoints must be an array");
  }
  if (input.length === 0) {
    return undefined;
  }
  const seen = new Set<string>();
  return input.map((entry, index) => {
    const endpoint = parseProxyEndpoint(entry, `proxyEndpoints[${index}]`);
    if (seen.has(endpoint.name)) {
      throw new Error(`proxyEndpoints duplicate name: ${endpoint.name}`);
    }
    seen.add(endpoint.name);
    return endpoint;
  });
}

function parseProxyEndpoint(input: unknown, path: string): PlatformProxyEndpoint {
  const value = requireRecord(input, path);
  const allowed = new Set([
    "name",
    "baseUrl",
    "authShape",
    "allowMethods",
    "allowPathPrefixes",
    "allowHeaders",
    "responseMode",
    "maxRequestBytes",
    "maxResponseBytes",
    "timeoutMs",
    "perCallBudget",
    "responseByteBudget"
  ]);
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) {
      throw new Error(`${path}.${key} is not an allowed field`);
    }
  }
  const name = requireString(value.name, `${path}.name`);
  if (!PROXY_ENDPOINT_NAME_PATTERN.test(name)) {
    throw new Error(
      `${path}.name must match ${PROXY_ENDPOINT_NAME_PATTERN} (lowercase letters, digits, '_' and '-'; <=63 chars)`
    );
  }
  if (RESERVED_PROXY_ENDPOINT_NAMES.has(name)) {
    throw new Error(`${path}.name is reserved: ${name}`);
  }
  const baseUrl = parseProxyBaseUrl(value.baseUrl, `${path}.baseUrl`);
  const authShape = parseProxyAuthShape(value.authShape, `${path}.authShape`);
  const allowMethods = parseProxyMethods(value.allowMethods, `${path}.allowMethods`);
  const allowPathPrefixes = parseProxyPathPrefixes(value.allowPathPrefixes, `${path}.allowPathPrefixes`);
  const allowHeaders = parseProxyAllowedHeaders(value.allowHeaders, `${path}.allowHeaders`, authShape);
  const responseMode = optionalEnum(
    value.responseMode,
    `${path}.responseMode`,
    PROXY_RESPONSE_MODES
  );
  const maxRequestBytes = optionalPositiveInt(value.maxRequestBytes, `${path}.maxRequestBytes`);
  const maxResponseBytes = optionalPositiveInt(value.maxResponseBytes, `${path}.maxResponseBytes`);
  const timeoutMs = optionalPositiveInt(value.timeoutMs, `${path}.timeoutMs`);
  const perCallBudget = optionalPositiveInt(value.perCallBudget, `${path}.perCallBudget`);
  const responseByteBudget = optionalPositiveInt(value.responseByteBudget, `${path}.responseByteBudget`);

  return {
    name,
    baseUrl,
    authShape,
    allowMethods,
    allowPathPrefixes,
    ...(allowHeaders ? { allowHeaders } : {}),
    ...(responseMode ? { responseMode } : {}),
    ...(maxRequestBytes !== undefined ? { maxRequestBytes } : {}),
    ...(maxResponseBytes !== undefined ? { maxResponseBytes } : {}),
    ...(timeoutMs !== undefined ? { timeoutMs } : {}),
    ...(perCallBudget !== undefined ? { perCallBudget } : {}),
    ...(responseByteBudget !== undefined ? { responseByteBudget } : {})
  };
}

function parseProxyBaseUrl(input: unknown, field: string): string {
  const raw = requireString(input, field);
  let parsed: URL;
  try {
    parsed = new URL(raw);
  } catch {
    throw new Error(`${field} must be a valid absolute URL`);
  }
  if (parsed.protocol !== "https:") {
    throw new Error(`${field} must use https://`);
  }
  if (parsed.username || parsed.password) {
    throw new Error(`${field} must not embed credentials`);
  }
  if (parsed.search || parsed.hash) {
    throw new Error(`${field} must not include a query string or fragment`);
  }
  // Normalize: strip trailing slash so prefix matching is predictable.
  const normalized = `${parsed.origin}${parsed.pathname.replace(/\/+$/, "")}`;
  return normalized;
}

function parseProxyAuthShape(input: unknown, field: string): ProxyAuthShape {
  const value = requireRecord(input, field);
  const type = requireString(value.type, `${field}.type`);
  switch (type as ProxyAuthType) {
    case "none":
      assertOnlyKeys(value, field, ["type"]);
      return { type: "none" };
    case "bearer":
      assertOnlyKeys(value, field, ["type"]);
      return { type: "bearer" };
    case "basic":
      assertOnlyKeys(value, field, ["type"]);
      return { type: "basic" };
    case "header": {
      assertOnlyKeys(value, field, ["type", "name"]);
      const name = requireString(value.name, `${field}.name`);
      assertHeaderName(name, `${field}.name`);
      return { type: "header", name };
    }
    case "query": {
      assertOnlyKeys(value, field, ["type", "name"]);
      const name = requireString(value.name, `${field}.name`);
      if (!/^[a-zA-Z0-9_\-.]{1,64}$/.test(name)) {
        throw new Error(`${field}.name must be a URL-safe identifier (<=64 chars)`);
      }
      return { type: "query", name };
    }
    default:
      throw new Error(`${field}.type must be one of: none, bearer, basic, header, query`);
  }
}

function parseProxyMethods(input: unknown, field: string): readonly ProxyMethod[] {
  if (!Array.isArray(input) || input.length === 0) {
    throw new Error(`${field} must be a non-empty array of HTTP methods`);
  }
  const seen = new Set<ProxyMethod>();
  for (const entry of input) {
    if (typeof entry !== "string") {
      throw new Error(`${field} entries must be strings`);
    }
    const upper = entry.toUpperCase() as ProxyMethod;
    if (!PROXY_ALLOWED_METHODS.includes(upper)) {
      throw new Error(`${field} contains unsupported method: ${entry}`);
    }
    seen.add(upper);
  }
  return Array.from(seen);
}

function parseProxyPathPrefixes(input: unknown, field: string): readonly string[] {
  if (!Array.isArray(input) || input.length === 0) {
    throw new Error(`${field} must be a non-empty array of path prefixes`);
  }
  const seen = new Set<string>();
  for (const entry of input) {
    if (typeof entry !== "string" || !entry.startsWith("/")) {
      throw new Error(`${field} entries must be non-empty strings starting with '/'`);
    }
    // Reject traversal / encoded traversal at config time so we never
    // need to second-guess at request time.
    if (entry.includes("..") || entry.toLowerCase().includes("%2e%2e")) {
      throw new Error(`${field} entry must not contain path traversal: ${entry}`);
    }
    seen.add(entry);
  }
  return Array.from(seen);
}

function parseProxyAllowedHeaders(
  input: unknown,
  field: string,
  authShape: ProxyAuthShape
): readonly string[] | undefined {
  if (input === undefined) {
    return undefined;
  }
  if (!Array.isArray(input)) {
    throw new Error(`${field} must be an array of header names`);
  }
  const seen = new Set<string>();
  const result: string[] = [];
  for (const entry of input) {
    if (typeof entry !== "string" || entry.length === 0) {
      throw new Error(`${field} entries must be non-empty strings`);
    }
    const lower = entry.toLowerCase();
    assertHeaderName(entry, field);
    if (PROXY_DENY_HEADER_LIST.has(lower)) {
      throw new Error(`${field} contains a forbidden header: ${entry}`);
    }
    const authHeader = authShapeHeaderName(authShape);
    if (authHeader && lower === authHeader) {
      throw new Error(
        `${field} must not contain the auth header for this endpoint (${authHeader}); the proxy injects it from secrets.proxyEndpointAuth`
      );
    }
    if (seen.has(lower)) {
      continue;
    }
    seen.add(lower);
    result.push(lower);
  }
  return result;
}

function assertHeaderName(value: string, field: string): void {
  // RFC 7230 token chars, conservative.
  if (!/^[A-Za-z0-9!#$%&'*+\-.^_`|~]{1,64}$/.test(value)) {
    throw new Error(`${field} must be a valid header token (<=64 chars): ${value}`);
  }
}

function assertOnlyKeys(value: Record<string, unknown>, field: string, allowed: readonly string[]): void {
  const permitted = new Set(allowed);
  for (const key of Object.keys(value)) {
    if (!permitted.has(key)) {
      throw new Error(`${field}.${key} is not an allowed field; permitted: ${allowed.join(", ")}`);
    }
  }
}

function crossValidateProxyEndpointsAndAuth(
  endpoints: readonly PlatformProxyEndpoint[] | undefined,
  auth: readonly PlatformProxyEndpointAuth[] | undefined
): void {
  const endpointsList = endpoints ?? [];
  const authList = auth ?? [];

  const endpointsByName = new Map(endpointsList.map((e) => [e.name, e]));
  const authByName = new Map(authList.map((a) => [a.name, a]));

  for (const endpoint of endpointsList) {
    const authEntry = authByName.get(endpoint.name);
    if (endpoint.authShape.type === "none") {
      // Keyless endpoints carry no auth value. Reject any matching
      // auth entry so callers don't accidentally ship a secret bound
      // to a "none" endpoint (which would be silently ignored at
      // request time — confusing and a leak risk).
      if (authEntry) {
        throw new Error(
          `proxyEndpoints[${endpoint.name}] has authShape "none" but a matching secrets.proxyEndpointAuth entry was supplied; remove the auth entry`
        );
      }
      continue;
    }
    if (!authEntry) {
      throw new Error(
        `proxyEndpoints[${endpoint.name}] has no matching secrets.proxyEndpointAuth entry`
      );
    }
    if (authEntry.value.type !== endpoint.authShape.type) {
      throw new Error(
        `secrets.proxyEndpointAuth[${endpoint.name}].value.type must equal proxyEndpoints[${endpoint.name}].authShape.type (expected ${endpoint.authShape.type}, got ${authEntry.value.type})`
      );
    }
  }

  for (const authEntry of authList) {
    if (!endpointsByName.has(authEntry.name)) {
      throw new Error(
        `secrets.proxyEndpointAuth[${authEntry.name}] has no matching proxyEndpoints entry`
      );
    }
  }
}

function parseCleanupPolicy(input: unknown): PlatformCleanupPolicy | undefined {
  if (input === undefined) {
    return undefined;
  }
  const value = requireRecord(input, "cleanup");
  const session = optionalEnum(value.session, "cleanup.session", ["retain", "delete"]);
  if (session === undefined) {
    return undefined;
  }
  return { session };
}

const PROVIDER_SECRET_KEYS = ["anthropic", "deepseek", "openai", "gemini", "mistral"] as const;

function parseInlineSecrets(input: unknown): PlatformInlineSecrets {
  const value = requireRecord(input, "secrets");
  const allowedTopLevel = new Set<string>([
    ...PROVIDER_SECRET_KEYS,
    "mcpServers",
    "proxyEndpointAuth"
  ]);
  for (const key of Object.keys(value)) {
    if (key.startsWith("__antpath_")) {
      // Platform-internal namespace (e.g. __antpath_proxy_token). The BFF
      // mutates the vaulted bundle to inject these; inbound submissions
      // are never allowed to set them, to prevent a malicious caller
      // from forging the bearer.
      throw new Error(
        `secrets.${key} uses the platform-internal __antpath_ namespace and may not be set by callers`
      );
    }
    if (!allowedTopLevel.has(key)) {
      throw new Error(
        `secrets.${key} is not an allowed field; permitted: ${[...allowedTopLevel].join(", ")}`
      );
    }
  }
  const anthropic =
    value.anthropic !== undefined ? parseProviderSecret(value.anthropic, "anthropic") : undefined;
  const deepseek =
    value.deepseek !== undefined ? parseProviderSecret(value.deepseek, "deepseek") : undefined;
  const openai =
    value.openai !== undefined ? parseProviderSecret(value.openai, "openai") : undefined;
  const gemini =
    value.gemini !== undefined ? parseProviderSecret(value.gemini, "gemini") : undefined;
  const mistral =
    value.mistral !== undefined ? parseProviderSecret(value.mistral, "mistral") : undefined;
  const mcpServers = parseMcpServerSecrets(value.mcpServers);
  const proxyEndpointAuth = parseProxyEndpointAuth(value.proxyEndpointAuth);

  return {
    ...(anthropic ? { anthropic } : {}),
    ...(deepseek ? { deepseek } : {}),
    ...(openai ? { openai } : {}),
    ...(gemini ? { gemini } : {}),
    ...(mistral ? { mistral } : {}),
    ...(mcpServers ? { mcpServers } : {}),
    ...(proxyEndpointAuth ? { proxyEndpointAuth } : {})
  };
}

function parseProviderSecret(
  input: unknown,
  provider: RunProvider
): { apiKey: string; baseUrl?: string } {
  const field = `secrets.${provider}`;
  const value = requireRecord(input, field);
  const allowed = new Set(["apiKey", "baseUrl"]);
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) {
      throw new Error(`${field}.${key} is not an allowed field; permitted: apiKey, baseUrl`);
    }
  }
  const apiKey = requireString(value.apiKey, `${field}.apiKey`);
  const rawBaseUrl = optionalString(value.baseUrl, `${field}.baseUrl`);
  if (rawBaseUrl === undefined) {
    return { apiKey };
  }
  // Reuse the proxy-endpoint URL guard so provider baseUrl gets the
  // same protection: https-only, no credentials, no query/fragment.
  // The provider-proxy in the dashboard forwards a customer-controlled
  // baseUrl to the upstream — accepting http:// (or a userinfo-laden
  // URL) here is an SSRF / credential-leak vector.
  const baseUrl = parseProxyBaseUrl(rawBaseUrl, `${field}.baseUrl`);
  return { apiKey, baseUrl };
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
  const allowed = new Set(["name", "url", "headers"]);
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) {
      throw new Error(`${path}.${key} is not an allowed field; permitted: name, url, headers`);
    }
  }
  const name = requireString(value.name, `${path}.name`);
  const url = requireString(value.url, `${path}.url`);
  const headers = optionalStringRecord(value.headers, `${path}.headers`);
  return headers ? { name, url, headers } : { name, url };
}

function parseProxyEndpointAuth(input: unknown): readonly PlatformProxyEndpointAuth[] | undefined {
  if (input === undefined) {
    return undefined;
  }
  if (!Array.isArray(input)) {
    throw new Error("secrets.proxyEndpointAuth must be an array");
  }
  if (input.length === 0) {
    return undefined;
  }
  const seen = new Set<string>();
  return input.map((entry, index) => {
    const auth = parseProxyEndpointAuthEntry(entry, `secrets.proxyEndpointAuth[${index}]`);
    if (seen.has(auth.name)) {
      throw new Error(`secrets.proxyEndpointAuth duplicate name: ${auth.name}`);
    }
    seen.add(auth.name);
    return auth;
  });
}

function parseProxyEndpointAuthEntry(input: unknown, path: string): PlatformProxyEndpointAuth {
  const value = requireRecord(input, path);
  const allowed = new Set(["name", "value"]);
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) {
      throw new Error(`${path}.${key} is not an allowed field; permitted: name, value`);
    }
  }
  const name = requireString(value.name, `${path}.name`);
  if (!PROXY_ENDPOINT_NAME_PATTERN.test(name)) {
    throw new Error(
      `${path}.name must match the same pattern as proxyEndpoints[].name (lowercase letters, digits, '_' and '-'; <=63 chars)`
    );
  }
  const valueField = parseProxyAuthValue(value.value, `${path}.value`);
  return { name, value: valueField };
}

function parseProxyAuthValue(input: unknown, path: string): PlatformProxyAuthValue {
  const value = requireRecord(input, path);
  const type = requireString(value.type, `${path}.type`);
  switch (type as PlatformProxyAuthValue["type"]) {
    case "bearer": {
      assertOnlyKeys(value, path, ["type", "token"]);
      const token = requireSecretValue(value.token, `${path}.token`);
      return { type: "bearer", token };
    }
    case "basic": {
      assertOnlyKeys(value, path, ["type", "username", "password"]);
      // Usernames are not redactable in the strict sense (often public
      // identifiers like an email), so we only enforce non-emptiness.
      // The password is the secret-bearing half.
      const username = requireString(value.username, `${path}.username`);
      const password = requireSecretValue(value.password, `${path}.password`);
      return { type: "basic", username, password };
    }
    case "header": {
      assertOnlyKeys(value, path, ["type", "value"]);
      const headerValue = requireSecretValue(value.value, `${path}.value`);
      return { type: "header", value: headerValue };
    }
    case "query": {
      assertOnlyKeys(value, path, ["type", "value"]);
      const queryValue = requireSecretValue(value.value, `${path}.value`);
      return { type: "query", value: queryValue };
    }
    default:
      throw new Error(`${path}.type must be one of: bearer, basic, header, query`);
  }
}

/**
 * The proxy body-redactor refuses to mask any derived target string shorter
 * than this many bytes — masking a 1-byte literal would corrupt the response
 * body. This is the floor for the *derived* redaction targets (e.g.
 * `Bearer <token>`, base64 fragments), used by
 * the hosted proxy redactor, which imports this constant so the two sides can
 * never silently diverge.
 */
export const MIN_REDACTION_TARGET_BYTES = 4;

/**
 * Minimum byte length for an accepted proxy secret *value*. Strictly greater
 * than {@link MIN_REDACTION_TARGET_BYTES}: a secret short enough to fall under
 * the redactor's floor could slip through unmasked, so the submission parser
 * rejects it up front. The `satisfies` check below pins that invariant at
 * compile time.
 */
const MIN_PROXY_SECRET_BYTES = 8;
// Invariant: an accepted secret must always be long enough for the redactor to
// mask it. If someone lowers MIN_PROXY_SECRET_BYTES below the redaction floor,
// this errors at compile time.
const _MIN_PROXY_SECRET_BYTES_OK: true =
  (MIN_PROXY_SECRET_BYTES >= MIN_REDACTION_TARGET_BYTES) as true;
void _MIN_PROXY_SECRET_BYTES_OK;

function requireSecretValue(input: unknown, field: string): string {
  const value = requireString(input, field);
  const byteLen = Buffer.byteLength(value, "utf8");
  if (byteLen < MIN_PROXY_SECRET_BYTES) {
    throw new Error(
      `${field} must be at least ${MIN_PROXY_SECRET_BYTES} bytes; shorter values cannot be reliably redacted from upstream responses`
    );
  }
  return value;
}

function assertNoSecretBearingFields(input: unknown, path: readonly string[]): void {
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

function requireRecord(input: unknown, field: string): Record<string, unknown> {
  if (!isRecord(input)) {
    throw new Error(`${field} must be an object`);
  }
  return input;
}

function isRecord(input: unknown): input is Record<string, unknown> {
  return typeof input === "object" && input !== null && !Array.isArray(input);
}

function requireString(input: unknown, field: string): string {
  if (typeof input !== "string" || input.length === 0) {
    throw new Error(`${field} must be a non-empty string`);
  }
  return input;
}

function optionalString(input: unknown, field: string): string | undefined {
  if (input === undefined) {
    return undefined;
  }
  return requireString(input, field);
}

function optionalEnum<const T extends readonly string[]>(input: unknown, field: string, allowed: T): T[number] | undefined {
  if (input === undefined) {
    return undefined;
  }
  if (typeof input !== "string" || !allowed.includes(input)) {
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

function optionalPositiveInt(input: unknown, field: string): number | undefined {
  if (input === undefined) {
    return undefined;
  }
  if (typeof input !== "number" || !Number.isSafeInteger(input) || input <= 0) {
    throw new Error(`${field} must be a positive safe integer`);
  }
  return input;
}

function isJsonValue(input: unknown): input is JsonValue {
  if (typeof input === "number") {
    return Number.isFinite(input);
  }
  if (input === null || typeof input === "string" || typeof input === "boolean") {
    return true;
  }
  if (Array.isArray(input)) {
    return input.every(isJsonValue);
  }
  if (isRecord(input)) {
    return Object.values(input).every(isJsonValue);
  }
  return false;
}

// ===========================================================================
// Run submission submission wire shape
// ===========================================================================

/**
 * Wire-level submission posted to /api/runs in the flat surface. The
 * `prompt` is always an array internally so the worker, the audit log,
 * and the BFF idempotency hash all see one shape. `mcpServers` carries
 * only the non-secret half; bearer headers travel in
 * `secrets.mcpServers` keyed by `name`.
 *
 * `skills` is a list of `SkillRef`s — workspace refs point at
 * `skill_bundles.id` (validated by the BFF before acceptance and pinned
 * into `run_skill_snapshots`), provider refs pass through unchanged.
 */
export interface PlatformSubmission {
  readonly model: string;
  readonly system?: string;
  readonly prompt: readonly string[];
  readonly skills: readonly SkillRef[];
  readonly agentsMd: readonly AgentsMdRef[];
  readonly files: readonly FileRef[];
  readonly mcpServers: readonly McpServerRef[];
  readonly environment?: PlatformEnvironment;
  readonly securityProfile?: RuntimeSecurityProfileName;
  readonly metadata?: Record<string, JsonValue>;
  /**
   * Opt-in container paths to capture as `output_objects` at session
   * terminal. When omitted, the worker still persists run metadata
   * (status, events, snapshots, cleanup state) but does not capture
   * any container file bytes. When present, the platform drives a
   * synthetic agent turn at session terminal that instructs the agent
   * to register every file under these paths via the Anthropic Files
   * API, then walks the resulting list and copies bytes into private
   * object storage.
   *
   * Validation:
   *   - Absolute UNIX paths only (starts with `/`).
   *   - No `..` segments, no NUL bytes, no embedded newlines.
   *   - Max 32 entries.
   *   - Max 512 bytes per entry.
   *
   * Entries are normalised (collapse `/+`, drop trailing `/` except
   * for `/`) and deduplicated. The normalised list is what travels in
   * the idempotency hash and the run snapshot.
   */
  readonly outputDirs?: readonly string[];
  /**
   * Optional override for the Goose builtin extensions enabled inside
   * the runner container. Each entry is the bare name accepted by
   * `goose run --with-builtin <NAME>` (see Goose v1.34.1's
   * `crates/goose-cli/src/cli.rs` `with-builtin` flag). The platform
   * default is `["developer"]` which gives the agent shell + write +
   * edit + tree tools (bash, grep via shell, file read via shell or
   * editor, file edit). To opt in to more tools (e.g. web search via
   * the `computercontroller` extension), pass the full list. To opt
   * out of all builtins (pure-MCP setup), pass an empty array.
   *
   * Validation:
   *   - Each entry matches /^[a-z][a-z0-9_-]{0,63}$/ (Goose builtin
   *     naming convention).
   *   - Max 16 entries.
   *   - Deduplicated.
   *
   * Anthropic Native runs ignore this field (they don't spawn Goose);
   * the dispatcher accepts and persists it for snapshot fidelity but
   * the Anthropic Native adapter never reads it.
   */
  readonly builtins?: readonly string[];
  /**
   * Platform-injection controls. The platform prepends a small system
   * prompt (see `platformSystemPrompt`) ahead of `system` so a bare
   * "save xx to the output dir" request works without the caller wiring
   * anything. Set `systemPrompt: "off"` to suppress that injection and
   * have the runtime see only the customer's own `system`. Omitting the
   * field (or `systemPrompt: "default"`) keeps the injection on.
   *
   * The default-output-directory behaviour is NOT governed by this flag —
   * an omitted `outputDirs` still falls back to the runtime's native
   * default capture directory regardless.
   */
  readonly platform?: PlatformInjectionConfig;
}

export interface PlatformInjectionConfig {
  readonly systemPrompt?: "default" | "off";
}

export interface PlatformRunSubmissionRequest {
  readonly workspaceId: string;
  readonly idempotencyKey: string;
  /**
   * Credential source for upstream provider access. Omitted means
   * `"byok"` for compatibility with the current production path.
   * `"managed"` is a public contract value but remains fail-closed in this
   * repository until a managed-key service supplies credential
   * resolution and billing admission.
   */
  readonly credentialMode: CredentialMode;
  /**
   * Provider selector. Always populated after parsing — absent on the
   * wire means {@link DEFAULT_RUN_PROVIDER}. The runtime selector
   * ({@link selectRuntime}) decides which runtime the run is dispatched
   * to.
   */
  readonly provider: RunProvider;
  /**
   * Customer's explicit runtime choice. `undefined` (the default) lets
   * {@link selectRuntime} auto-route based on `provider`. When set,
   * `parseRunSubmissionRequest` already verified that the choice is
   * compatible with `provider`; the dispatcher still re-validates
   * feature compatibility before enqueueing.
   */
  readonly runtime?: RuntimeKind;
  readonly submission: PlatformSubmission;
  readonly cleanup?: PlatformCleanupPolicy;
  readonly secrets: PlatformInlineSecrets;
  readonly proxyEndpoints?: readonly PlatformProxyEndpoint[];
  /**
   * Goose Fly-machine size. One of the closed {@link MachineSize} preset
   * tokens or absent (⇒ {@link DEFAULT_MACHINE_SIZE}). Native (Anthropic
   * Managed Agents) runs have no Fly machine and ignore this field; the
   * dispatcher still accepts + persists it for snapshot fidelity.
   */
  readonly machine?: MachineSize;
  /**
   * Run deadline in milliseconds, normalised by the parser from the wire
   * `timeout` duration string (bounded to [1m, 6h]). Absent ⇒
   * {@link DEFAULT_RUN_TIMEOUT_MS} (1h). Applies to BOTH runtimes — Goose's
   * `waitForEvent` window + the runner self-kill, and the native poll
   * deadline.
   */
  readonly timeoutMs?: number;
}

/**
 * Wire shape posted by the SDK and CLI. `workspaceId` is **omitted by
 * design** — token-authenticated clients never name the workspace
 * because it is derived from their API token on the server. The BFF
 * route resolves the workspace from the token and injects it before
 * calling the parser. The dashboard UI (Auth.js user principal,
 * multi-workspace) is the only caller that supplies `workspaceId`
 * itself.
 *
 * `provider` is also optional on the wire — absent means
 * {@link DEFAULT_RUN_PROVIDER} (`anthropic`). The parser fills it in
 * before the value enters the run snapshot.
 */
export type PlatformRunSubmissionInput = Omit<
  PlatformRunSubmissionRequest,
  "workspaceId" | "credentialMode" | "provider" | "runtime" | "timeoutMs"
> & {
  readonly workspaceId?: string;
  readonly credentialMode?: CredentialMode;
  readonly provider?: RunProvider;
  /**
   * Optional runtime opt-out. Set `"managed"` to force the Goose
   * Managed runtime (rejects features that only work natively).
   * `"native"` is only valid when `provider === "anthropic"`.
   * Absent = auto-route. See {@link selectRuntime}.
   */
  readonly runtime?: RuntimeKind;
  /**
   * Run deadline as a human duration string (`"1h"`, `"90m"`, `"30s"`).
   * Parsed + bounded to [1m, 6h] server-side into
   * {@link PlatformRunSubmissionRequest.timeoutMs}. Absent ⇒ 1h default.
   */
  readonly timeout?: string;
};

export interface ParseRunSubmissionOptions {
  readonly managedKeyPolicy?: ManagedKeyPolicyV1;
}

export function parseRunSubmissionRequest(
  input: unknown,
  options: ParseRunSubmissionOptions = {}
): PlatformRunSubmissionRequest {
  const value = requireRecord(input, "submission");
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
  const provider = parseRunProvider(value.provider);
  const runtime = parseRuntimeKind(value.runtime);
  const credentialMode = parseCredentialMode(value.credentialMode);
  if (credentialMode === "managed") {
    assertManagedKeyAdmissionAllowed(options.managedKeyPolicy);
  }
  // Cross-field validation via the centralized runtime-support validator.
  const runtimeSupport = checkRuntimeSupported(provider, runtime);
  if (!runtimeSupport.ok) {
    throw new RuntimeValidationError(runtimeSupport.code!, runtimeSupport.message!);
  }
  const cleanup = parseCleanupPolicy(value.cleanup);
  const machine = parseMachineSize(value.machine);
  const timeoutMs = parseRunTimeout(value.timeout);
  const proxyEndpoints = parseProxyEndpoints(value.proxyEndpoints);
  const secrets = parseInlineSecrets(value.secrets);
  enforceCredentialSecretPolicy(provider, credentialMode, secrets);

  crossValidateProxyEndpointsAndAuth(proxyEndpoints, secrets.proxyEndpointAuth);

  const submission = parseSubmission(value.submission);

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

  // Cross-field feature/runtime validation. When the caller explicitly
  // opted into runtime: "managed", they must not also request any
  // native-only feature (provider built-in skills, etc.). Done HERE in
  // the parser so every entry-point (Worker, dashboard BFF, SDK) gets
  // the same fail-closed behaviour. Previously this lived only in the
  // separately-invokable selectRuntime() — and the dashboard BFF
  // forgot to invoke it, letting incoherent runs land in the DB.
  if (runtime === "managed") {
    const candidate: PlatformRunSubmissionRequest = {
      workspaceId: "",
      idempotencyKey: "",
      credentialMode,
      provider,
      runtime,
      submission,
      secrets
    };
    const nativeOnly = collectNativeOnlyFeatures(candidate);
    if (nativeOnly.length > 0) {
      throw new RuntimeValidationError(
        "feature_runtime_mismatch",
        `runtime: "managed" rejects the following features: ${nativeOnly.join(", ")}. ` +
          `Remove them or switch to runtime: "native".`
      );
    }
  }

  return {
    workspaceId: requireString(value.workspaceId, "workspaceId"),
    idempotencyKey: requireString(value.idempotencyKey, "idempotencyKey"),
    credentialMode,
    provider,
    ...(runtime ? { runtime } : {}),
    submission,
    ...(cleanup ? { cleanup } : {}),
    ...(machine ? { machine } : {}),
    ...(timeoutMs !== undefined ? { timeoutMs } : {}),
    ...(proxyEndpoints ? { proxyEndpoints } : {}),
    secrets
  };
}

function parseRuntimeKind(input: unknown): RuntimeKind | undefined {
  if (input === undefined) {
    return undefined;
  }
  if (typeof input !== "string" || !(RUNTIME_KINDS as readonly string[]).includes(input)) {
    throw new Error(
      `runtime must be one of: ${RUNTIME_KINDS.join(", ")} (got ${JSON.stringify(input)})`
    );
  }
  return input as RuntimeKind;
}

function isNativeRuntimeProvider(provider: RunProvider): boolean {
  // Capability registry is the source of truth (native-first dispatch);
  // NATIVE_RUNTIME_PROVIDERS is the error-string mirror, asserted equal
  // by a unit test so the two cannot drift.
  return providerHasNativeAgent(provider);
}

function formatProviderList(providers: readonly RunProvider[]): string {
  return providers.map((p) => `"${p}"`).join(", ");
}

function parseRunProvider(input: unknown): RunProvider {
  if (input === undefined) {
    return DEFAULT_RUN_PROVIDER;
  }
  if (typeof input !== "string" || !(RUN_PROVIDERS as readonly string[]).includes(input)) {
    throw new Error(
      `provider must be one of: ${RUN_PROVIDERS.join(", ")} (got ${JSON.stringify(input)})`
    );
  }
  return input as RunProvider;
}

/**
 * Cross-check the chosen provider against the supplied secrets bundle.
 *
 *  - The matching provider's apiKey MUST be present.
 *  - Every OTHER provider's secret block MUST be absent (cross-provider
 *    secrets are explicitly rejected, not silently dropped — they are
 *    almost always a copy-paste mistake or a confused caller, and we
 *    want to fail loud).
 *  - MCP / proxy endpoint auth carry across providers and are not
 *    checked here.
 */
function enforceCredentialSecretPolicy(
  provider: RunProvider,
  credentialMode: CredentialMode,
  secrets: PlatformInlineSecrets
): void {
  if (credentialMode === "managed") {
    for (const providerKey of PROVIDER_SECRET_KEYS) {
      if (secrets[providerKey] !== undefined) {
        throw new Error(
          `secrets.${providerKey} is not allowed when credentialMode is managed; provider access is resolved by the managed-key policy`
        );
      }
    }
    return;
  }

  const required = secrets[provider];
  if (!required?.apiKey) {
    throw new Error(`secrets.${provider}.apiKey is required when provider is ${provider}`);
  }
  for (const other of PROVIDER_SECRET_KEYS) {
    if (other === provider) {
      continue;
    }
    if (secrets[other] !== undefined) {
      throw new Error(
        `secrets.${other} is not allowed when provider is ${provider}; remove it or set provider to ${other}`
      );
    }
  }
}

function parseSubmission(input: unknown): PlatformSubmission {
  const value = requireRecord(input, "submission.submission");
  const allowed = new Set([
    "model",
    "system",
    "prompt",
    "skills",
    "agentsMd",
    "files",
    "mcpServers",
    "environment",
    "securityProfile",
    "metadata",
    "outputDirs",
    "builtins",
    "platform"
  ]);
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) {
      throw new Error(`submission.${key} is not an allowed field; permitted: ${[...allowed].join(", ")}`);
    }
  }
  const model = requireString(value.model, "submission.model");
  const system = optionalString(value.system, "submission.system");
  const prompt = parsePrompt(value.prompt);
  const skills = parseSkills(value.skills);
  const agentsMd = parseAgentsMd(value.agentsMd);
  const files = parseFiles(value.files);
  const mcpServers = parseMcpServers(value.mcpServers);
  const environment = parseEnvironment(value.environment);
  const securityProfile = parseRuntimeSecurityProfile(value.securityProfile);
  const metadata = optionalJsonRecord(value.metadata, "submission.metadata");
  const outputDirs = parseOutputDirs(value.outputDirs);
  const builtins = parseBuiltins(value.builtins);
  const platform = parsePlatformConfig(value.platform);

  return {
    model,
    ...(system ? { system } : {}),
    prompt,
    skills,
    agentsMd,
    files,
    mcpServers,
    ...(environment ? { environment } : {}),
    ...(securityProfile ? { securityProfile } : {}),
    ...(metadata ? { metadata } : {}),
    ...(outputDirs ? { outputDirs } : {}),
    ...(builtins !== undefined ? { builtins } : {}),
    ...(platform ? { platform } : {})
  };
}

function parsePlatformConfig(input: unknown): PlatformInjectionConfig | undefined {
  if (input === undefined || input === null) return undefined;
  const value = requireRecord(input, "submission.platform");
  for (const key of Object.keys(value)) {
    if (key !== "systemPrompt") {
      throw new Error(`submission.platform.${key} is not an allowed field; permitted: systemPrompt`);
    }
  }
  if (value.systemPrompt === undefined) return undefined;
  if (value.systemPrompt !== "default" && value.systemPrompt !== "off") {
    throw new Error(`submission.platform.systemPrompt must be "default" or "off"`);
  }
  return { systemPrompt: value.systemPrompt };
}

const BUILTIN_NAME_PATTERN = /^[a-z][a-z0-9_-]{0,63}$/;
const MAX_BUILTINS = 16;

function parseBuiltins(input: unknown): readonly string[] | undefined {
  if (input === undefined || input === null) return undefined;
  if (!Array.isArray(input)) {
    throw new Error("submission.builtins must be an array of strings");
  }
  if (input.length > MAX_BUILTINS) {
    throw new Error(`submission.builtins exceeds the max of ${MAX_BUILTINS} entries`);
  }
  const seen = new Set<string>();
  const out: string[] = [];
  for (let i = 0; i < input.length; i++) {
    const v = input[i];
    if (typeof v !== "string") {
      throw new Error(`submission.builtins[${i}] must be a string`);
    }
    if (!BUILTIN_NAME_PATTERN.test(v)) {
      throw new Error(
        `submission.builtins[${i}] (${JSON.stringify(v)}) is not a valid Goose builtin name; expected /^[a-z][a-z0-9_-]{0,63}$/`
      );
    }
    if (seen.has(v)) continue; // dedupe silently
    seen.add(v);
    out.push(v);
  }
  return out;
}

/**
 * Maximum number of `outputDirs` entries accepted per submission.
 *
 * 32 is enough room for the typical "one or two capture roots" pattern
 * plus a generous margin for legitimate multi-root use cases (per-tool
 * output directory + scratch state + logs, repeated across a few
 * subdirectories), without inviting abuse of the synthetic-turn path
 * the worker drives at session terminal.
 */
const MAX_OUTPUT_DIRS = 32;

/**
 * Maximum byte length of a single `outputDirs` entry (after UTF-8
 * encoding). 512 bytes comfortably covers `/very/long/nested/path`
 * style entries without letting a misuse smuggle large blobs through
 * the field.
 */
const MAX_OUTPUT_DIR_BYTES = 512;

function parseOutputDirs(input: unknown): readonly string[] | undefined {
  if (input === undefined) {
    return undefined;
  }
  if (!Array.isArray(input)) {
    throw new Error("submission.outputDirs must be an array of absolute UNIX paths");
  }
  if (input.length === 0) {
    // Treat an empty array as omission so the idempotency hash matches
    // the "no outputDirs" case.
    return undefined;
  }
  if (input.length > MAX_OUTPUT_DIRS) {
    throw new Error(
      `submission.outputDirs has ${input.length} entries; max is ${MAX_OUTPUT_DIRS}`
    );
  }
  const seen = new Set<string>();
  const normalised: string[] = [];
  for (let i = 0; i < input.length; i++) {
    const item = input[i];
    if (typeof item !== "string") {
      throw new Error(`submission.outputDirs[${i}] must be a string`);
    }
    if (item.length === 0) {
      throw new Error(`submission.outputDirs[${i}] must be a non-empty absolute UNIX path`);
    }
    const bytes = new TextEncoder().encode(item).length;
    if (bytes > MAX_OUTPUT_DIR_BYTES) {
      throw new Error(
        `submission.outputDirs[${i}] exceeds ${MAX_OUTPUT_DIR_BYTES} bytes (got ${bytes})`
      );
    }
    if (!item.startsWith("/")) {
      throw new Error(
        `submission.outputDirs[${i}] must be an absolute UNIX path (start with '/')`
      );
    }
    if (item.includes("\0")) {
      throw new Error(`submission.outputDirs[${i}] must not contain NUL bytes`);
    }
    if (item.includes("\n") || item.includes("\r")) {
      throw new Error(`submission.outputDirs[${i}] must not contain newline characters`);
    }
    const segments = item.split("/");
    if (segments.includes("..")) {
      throw new Error(`submission.outputDirs[${i}] must not contain '..' segments`);
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

function parseSkills(input: unknown): readonly SkillRef[] {
  if (input === undefined) {
    return [];
  }
  if (!Array.isArray(input)) {
    throw new Error("submission.skills must be an array of SkillRef objects");
  }
  const seenProvider = new Set<string>();
  const seenR2Path = new Set<string>();
  return input.map((item, index) => {
    const ref = parseSkillRef(item, `submission.skills[${index}]`);
    if (ref.kind === "provider") {
      const key = `${ref.vendor}:${ref.skillId}:${ref.version ?? ""}`;
      if (seenProvider.has(key)) {
        throw new Error(
          `submission.skills duplicate provider skill: ${ref.vendor}:${ref.skillId}${ref.version ? `:${ref.version}` : ""}`
        );
      }
      seenProvider.add(key);
    } else if (ref.kind === "r2") {
      if (seenR2Path.has(ref.path)) {
        throw new Error(`submission.skills duplicate r2 path: ${ref.path}`);
      }
      seenR2Path.add(ref.path);
    }
    return ref;
  });
}

function parseAgentsMd(input: unknown): readonly AgentsMdRef[] {
  if (input === undefined) return [];
  if (!Array.isArray(input)) {
    throw new Error("submission.agentsMd must be an array of AgentsMdRef objects");
  }
  const seenR2Path = new Set<string>();
  return input.map((item, index): AgentsMdRef => {
    const path = `submission.agentsMd[${index}]`;
    if (!item || typeof item !== "object" || Array.isArray(item)) {
      throw new Error(`${path} must be an AgentsMdRef object`);
    }
    const raw = item as Record<string, unknown>;
    if (raw.kind !== "r2") {
      throw new Error(`${path}.kind must be 'r2' (got ${JSON.stringify(raw.kind)})`);
    }
    const fields = parseR2RefFields(raw, path);
    if (seenR2Path.has(fields.path)) {
      throw new Error(`submission.agentsMd duplicate r2 path: ${fields.path}`);
    }
    seenR2Path.add(fields.path);
    return { kind: "r2", path: fields.path, hash: fields.hash, sizeBytes: fields.sizeBytes, name: fields.name };
  });
}

function parseFiles(input: unknown): readonly FileRef[] {
  if (input === undefined) return [];
  if (!Array.isArray(input)) {
    throw new Error("submission.files must be an array of FileRef objects");
  }
  const seenR2Path = new Set<string>();
  return input.map((item, index): FileRef => {
    const path = `submission.files[${index}]`;
    if (!item || typeof item !== "object" || Array.isArray(item)) {
      throw new Error(`${path} must be a FileRef object`);
    }
    const raw = item as Record<string, unknown>;
    if (raw.kind !== "r2") {
      throw new Error(`${path}.kind must be 'r2' (got ${JSON.stringify(raw.kind)})`);
    }
    const fields = parseR2RefFields(raw, path);
    if (seenR2Path.has(fields.path)) {
      throw new Error(`submission.files duplicate r2 path: ${fields.path}`);
    }
    seenR2Path.add(fields.path);
    if (fields.mountPath !== undefined && !fields.mountPath.startsWith("/")) {
      throw new Error(`${path}.mountPath must start with '/' if provided`);
    }
    return fields.mountPath !== undefined
      ? { kind: "r2", path: fields.path, hash: fields.hash, sizeBytes: fields.sizeBytes, name: fields.name, mountPath: fields.mountPath }
      : { kind: "r2", path: fields.path, hash: fields.hash, sizeBytes: fields.sizeBytes, name: fields.name };
  });
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

// ===========================================================================
// Runtime dispatcher
// ===========================================================================

/**
 * Codes the dispatcher emits when a submission can't be served by the
 * selected runtime. Surfaced both at parse time (cross-field shape
 * mismatch) and at dispatch time (feature mismatch). Code values are
 * stable so dashboard / SDK error rendering can branch on them.
 */
export const RUNTIME_VALIDATION_CODES = [
  "runtime_native_unsupported",
  "feature_runtime_mismatch"
] as const;
export type RuntimeValidationCode = (typeof RUNTIME_VALIDATION_CODES)[number];

/**
 * Thrown by `parseRunSubmissionRequest` (wire-shape mismatch) and by
 * `selectRuntime` (feature mismatch) when the submitted run cannot be
 * served by the chosen runtime. The `code` field is part of the public
 * contract — keep it stable when phrasings change.
 */
export class RuntimeValidationError extends Error {
  readonly code: RuntimeValidationCode;
  constructor(code: RuntimeValidationCode, message: string) {
    super(message);
    this.name = "RuntimeValidationError";
    this.code = code;
  }
}

/**
 * Walk the parsed submission and collect every feature that **only**
 * works on the Anthropic Native runtime. Today the only such feature
 * is a provider built-in skill ref (`Skill.provider(...)` — the
 * Anthropic Skills API is the resolver). Adding a new native-only
 * feature means extending this function AND the matching error string
 * in {@link selectRuntime}.
 *
 * Exported because Phase 5's runtime dispatcher route handler reuses
 * it to format the API error body and the dashboard reuses it to point
 * the user at the offending submission field.
 */
export function collectNativeOnlyFeatures(req: PlatformRunSubmissionRequest): string[] {
  const features: string[] = [];
  for (const skill of req.submission.skills) {
    if (skill.kind === "provider") {
      const versionSuffix = skill.version ? `, "${skill.version}"` : "";
      features.push(`Skill.provider("${skill.vendor}", "${skill.skillId}"${versionSuffix})`);
    }
  }
  return features;
}

/**
 * Walk the parsed submission and collect every feature the selected native
 * runtime does not serve according to {@link PROVIDER_CAPABILITY}. Anthropic's
 * native executor currently serves inline skills, files, and MCP servers, so
 * this returns an empty list for those public fields. Provider built-in skills
 * (`Skill.provider(...)`) are native-only and are gated in the managed path
 * by {@link collectNativeOnlyFeatures}.
 *
 * The customer-surface invariant is "every customer-facing field works in both runtimes OR is rejected
 * at submission time — no silent re-routing." This function is the
 * source of truth for the native half of that gate; {@link selectRuntime}
 * throws `feature_runtime_mismatch` when it returns a non-empty list, so
 * a run that would otherwise silently drop these features is refused with
 * an explicit pointer to `runtime: "managed"`.
 *
 * If a future native provider or deliberate capability change cannot serve a
 * field, update the registry; this function will fail closed from that fact.
 */
export function collectNativeUnsupportedFeatures(req: PlatformRunSubmissionRequest): string[] {
  // Capability-driven: only flag a feature when the provider's native runtime
  // does not serve it (`PROVIDER_CAPABILITY[...].serves`).
  const cap = PROVIDER_CAPABILITY[req.provider].nativeAgent;
  // No native runtime ⇒ the run never resolves to native, so there is
  // nothing for the native gate to reject.
  if (!cap) return [];
  const features: string[] = [];
  if (!cap.serves.inlineSkills) {
    req.submission.skills.forEach((skill, i) => {
      if (skill.kind !== "provider") {
        const name = "name" in skill && skill.name ? ` "${skill.name}"` : "";
        features.push(`skills[${i}] (inline skill${name})`);
      }
    });
  }
  if (!cap.serves.files) {
    req.submission.files.forEach((file, i) => {
      const name = "name" in file && file.name ? ` "${file.name}"` : "";
      features.push(`files[${i}]${name}`);
    });
  }
  if (!cap.serves.mcpServers) {
    req.submission.mcpServers.forEach((mcp, i) => {
      const name = "name" in mcp && mcp.name ? ` "${mcp.name}"` : "";
      features.push(`mcpServers[${i}]${name}`);
    });
  }
  return features;
}

/**
 * Throw `feature_runtime_mismatch` when a run resolved to native carries
 * features that runtime cannot serve. Used for both the explicit
 * `runtime: "native"` path and the auto-routed native path so neither silently
 * drops submitted fields.
 */
function assertNativeCanServe(req: PlatformRunSubmissionRequest): void {
  const unsupported = collectNativeUnsupportedFeatures(req);
  if (unsupported.length > 0) {
    throw new RuntimeValidationError(
      "feature_runtime_mismatch",
      `The selected native runtime does not support these submission features: ` +
        `${unsupported.join(", ")}. Submit with runtime: "managed" to run them on the ` +
        `Goose runtime, or remove them.`
    );
  }
}

/**
 * The runtime dispatcher. Pure function with no I/O — call it after
 * `parseRunSubmissionRequest` to decide which runtime serves the run.
 *
 * Native-first, driven by {@link PROVIDER_CAPABILITY}:
 *   1. Explicit `runtime` wins, validated against provider + feature set.
 *      `"native"` on a provider with no native agent runtime is rejected;
 *      `"managed"` (opt-out to Goose) rejects native-only features. No
 *      silent re-routing.
 *   2. Auto-route: a provider WITH a native agent runtime goes native
 *      (provider-hosted is the preferred path); a provider WITHOUT one
 *      falls back to Goose Managed (the universal fallback). Feature-level
 *      gaps within a native-capable provider stay fail-closed (the run is
 *      rejected, not silently re-routed) — that gate dissolves as the
 *      native `serves` flags flip.
 *
 * Returns {@link RuntimeKind} (`"native" | "managed"`); the concrete
 * native executor is resolved downstream from the provider.
 */
export function selectRuntime(req: PlatformRunSubmissionRequest): RuntimeKind {
  if (req.runtime === "native") {
    const support = checkRuntimeSupported(req.provider, "native");
    if (!support.ok) {
      throw new RuntimeValidationError(support.code!, support.message!);
    }
    assertNativeCanServe(req);
    return "native";
  }
  if (req.runtime === "managed") {
    const features = collectNativeOnlyFeatures(req);
    if (features.length > 0) {
      throw new RuntimeValidationError(
        "feature_runtime_mismatch",
        `runtime: "managed" rejects the following features: ${features.join(", ")}. ` +
          `Remove them or switch to runtime: "native".`
      );
    }
    return "managed";
  }
  // Auto-routing — no explicit `runtime`. Native-first: a provider with
  // a native agent runtime ({@link PROVIDER_CAPABILITY}) goes native;
  // everything else falls back to Goose Managed. The provider-level
  // fallback is automatic (no native runtime exists), but we do NOT
  // silently re-route a native-capable provider's run with
  // native-unsupported features to managed — that would violate the
  // no-auto-switching invariant — so the same fail-closed gate applies.
  if (isNativeRuntimeProvider(req.provider)) {
    assertNativeCanServe(req);
    return "native";
  }
  return "managed";
}
