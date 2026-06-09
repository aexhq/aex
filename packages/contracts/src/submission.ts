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
// existing `@aexhq/contracts` consumers of `PROXY_ENDPOINT_DEFAULTS` are
// unaffected by the move.
export { PROXY_ENDPOINT_DEFAULTS };
import { parseAssetRefFields, parseMcpServerRef, parseSkillRef } from "./run-config.js";
import type {
  AgentsMdRef,
  FileRef,
  McpServerRef,
  SkillRef
} from "./run-config.js";
import { parseRunTimeout, parseRuntimeSize, type RuntimeSize } from "./runtime-sizes.js";
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
 * managed container process and mirrored in the mounted `RUNTIME.env` /
 * `RUNTIME.json` files. The same keys become `__KEY__` substitution targets
 * in agent-facing markdown inside skill / agentsmd / file bundles. Aex-set
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
  /** Lowercase host names. The worker always appends the proxy host. */
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
 * Run-time provider selector. Aex exposes one customer interface
 * for every provider. All new submissions execute through the managed
 * runtime; provider selection only decides which upstream model route
 * the managed provider-proxy uses.
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
 * Customer-facing runtime selector. Optional on the wire; absent resolves
 * to the same managed runtime as `"managed"`. `"native"` is no longer an
 * accepted submission value and fails schema validation.
 */
export const RUNTIME_KINDS = ["managed"] as const;
export type RuntimeKind = (typeof RUNTIME_KINDS)[number];

/** Outcome of the centralized runtime-support check. */
export interface RuntimeSupportCheck {
  readonly ok: boolean;
  readonly message?: string;
}

/**
 * Centralized runtime-support validator. Native is removed from the public
 * runtime enum, so an absent runtime and `"managed"` are the only supported
 * inputs. Schema parsing rejects other runtime strings before this helper is
 * reached, but the result type remains for SDK preflight checks.
 */
export function checkRuntimeSupported(
  provider: RunProvider,
  runtime: RuntimeKind | undefined
): RuntimeSupportCheck {
  void provider;
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
 * Per-run inline secrets bundle. `apiKey` is the BYOK provider key for the
 * run's selected `provider` (required in `"byok"` credential mode, rejected
 * in `"managed"` mode). A run targets exactly one provider, so the key is a
 * single flat field rather than a per-provider block. `mcpServers` and
 * `proxyEndpointAuth` are cross-provider (an MCP credential is the same
 * secret whichever model is driving the MCP client).
 */
export interface PlatformInlineSecrets {
  readonly apiKey?: string;
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
}

export const SECRETS_KEY = "secrets";

export const PROXY_ENDPOINT_NAME_PATTERN = /^[a-z][a-z0-9_-]{0,62}$/;
export const RESERVED_PROXY_ENDPOINT_NAMES = new Set(["proxy", "aex", "internal", "admin"]);

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

export const deniedSecretFields = new Set([
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
 *   - Keys MUST NOT start with the reserved `AEX_` prefix; that
 *     prefix is owned by platform-set runtime keys and a collision
 *     would silently mask `__AEX_CLI__` etc. substitution
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
    if (key.startsWith(AEX_RESERVED_ENV_PREFIX)) {
      throw new Error(
        `submission.environment.envVars.${key} uses reserved prefix "${AEX_RESERVED_ENV_PREFIX}" (set by aex runtime)`
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
    "timeoutMs"
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
    ...(timeoutMs !== undefined ? { timeoutMs } : {})
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

export function parseProxyAuthShape(input: unknown, field: string): ProxyAuthShape {
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

export function parseProxyMethods(input: unknown, field: string): readonly ProxyMethod[] {
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

export function parseProxyPathPrefixes(input: unknown, field: string): readonly string[] {
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

export function parseProxyAllowedHeaders(
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

export function crossValidateProxyEndpointsAndAuth(
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

export function parseInlineSecrets(input: unknown): PlatformInlineSecrets {
  const value = requireRecord(input, "secrets");
  const allowedTopLevel = new Set<string>(["apiKey", "mcpServers", "proxyEndpointAuth"]);
  for (const key of Object.keys(value)) {
    if (key.startsWith("__aex_")) {
      // Platform-internal namespace (e.g. __aex_proxy_token). The BFF
      // mutates the vaulted bundle to inject these; inbound submissions
      // are never allowed to set them, to prevent a malicious caller
      // from forging the bearer.
      throw new Error(
        `secrets.${key} uses the platform-internal __aex_ namespace and may not be set by callers`
      );
    }
    if (!allowedTopLevel.has(key)) {
      throw new Error(
        `secrets.${key} is not an allowed field; permitted: ${[...allowedTopLevel].join(", ")}`
      );
    }
  }
  const apiKey =
    value.apiKey !== undefined ? requireString(value.apiKey, "secrets.apiKey") : undefined;
  const mcpServers = parseMcpServerSecrets(value.mcpServers);
  const proxyEndpointAuth = parseProxyEndpointAuth(value.proxyEndpointAuth);

  return {
    ...(apiKey !== undefined ? { apiKey } : {}),
    ...(mcpServers ? { mcpServers } : {}),
    ...(proxyEndpointAuth ? { proxyEndpointAuth } : {})
  };
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

function isRecord(input: unknown): input is Record<string, unknown> {
  return typeof input === "object" && input !== null && !Array.isArray(input);
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

export function optionalPositiveInt(input: unknown, field: string): number | undefined {
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
   * Output capture policy. Omit `outputs.allowedDirs` to capture the whole
   * filesystem delta; provide it to narrow capture to the listed roots.
   * `outputs.deniedDirs` subtracts denied roots/patterns from the allowed set.
   */
  readonly outputs?: PlatformOutputCaptureConfig;
  /**
   * Optional override for the managed-runtime builtin extensions enabled
   * inside the runner container. Each entry is the bare extension name
   * accepted by the selected runtime. The platform
   * default is `["developer"]` which gives the agent shell + write +
   * edit + tree tools (bash, grep via shell, file read via shell or
   * editor, file edit). To opt in to more tools (e.g. web search via
   * the `computercontroller` extension), pass the full list. To opt
   * out of all builtins (pure-MCP setup), pass an empty array.
   *
   * Validation:
   *   - Each entry matches /^[a-z][a-z0-9_-]{0,63}$/ (managed-runtime
   *     builtin naming convention).
   *   - Max 16 entries.
   *   - Deduplicated.
   *
   * The dispatcher accepts and persists it for snapshot fidelity.
   */
  readonly builtins?: readonly string[];
  /**
   * Assistant-output granularity. `buffered` (the default) emits one event per
   * assistant message; `stream` emits the agent's per-token text deltas as they
   * arrive. Buffered is quieter and cheaper; stream suits live typing UIs.
   */
  readonly outputMode?: OutputMode;
  /**
   * Platform-injection controls. The platform prepends a small system
   * prompt (see `platformSystemPrompt`) ahead of `system` to explain
   * managed-run expectations such as durable file capture. Set
   * `systemPrompt: "off"` to suppress that injection and have the runtime
   * see only the customer's own `system`. Omitting the field (or
   * `systemPrompt: "default"`) keeps the injection on.
   *
   * This does not change output capture scope. Omitted
   * `outputs.allowedDirs` means capture all created/modified files; explicit
   * `outputs.allowedDirs` narrows it.
   */
  readonly platform?: PlatformInjectionConfig;
}

export interface PlatformOutputCaptureConfig {
  /**
   * Allowed capture roots. Omit or pass an empty list to use the default
   * whole-filesystem delta capture. Entries are absolute UNIX paths.
   */
  readonly allowedDirs?: readonly string[];
  /**
   * Denied capture roots/patterns. These are subtracted from the allowed roots;
   * platform-mandatory denies always apply and cannot be re-included.
   */
  readonly deniedDirs?: readonly string[];
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
   * `"managed"` is a public contract value but remains fail-closed until
   * credential resolution and billing admission are available.
   */
  readonly credentialMode: CredentialMode;
  /**
   * Provider selector. Always populated after parsing — absent on the
   * wire means {@link DEFAULT_RUN_PROVIDER}. All providers are dispatched
   * through the managed runtime.
   */
  readonly provider: RunProvider;
  /**
   * Customer's explicit runtime choice. `undefined` and `"managed"` both
   * resolve to the managed runtime. Other runtime values are rejected by
   * `parseRunSubmissionRequest`.
   */
  readonly runtime?: RuntimeKind;
  readonly submission: PlatformSubmission;
  readonly secrets: PlatformInlineSecrets;
  readonly proxyEndpoints?: readonly PlatformProxyEndpoint[];
  /**
   * Managed runtime size. One of the closed {@link RuntimeSize} preset tokens
   * or absent (downstream applies the default).
   */
  readonly runtimeSize?: RuntimeSize;
  /**
   * Run deadline in milliseconds, normalised by the parser from the wire
   * `timeout` duration string (bounded to [1m, 6h]). Absent ⇒
   * {@link DEFAULT_RUN_TIMEOUT_MS} (1h). Applies to the managed runner's
   * terminal wait window and self-kill deadline.
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
   * Optional runtime selector. Set `"managed"` explicitly or omit the
   * field; both resolve to the managed runtime. `"native"` is no longer
   * accepted.
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
  const allowedTopLevelFields = new Set([
    "workspaceId",
    "idempotencyKey",
    "credentialMode",
    "provider",
    "runtime",
    "submission",
    "runtimeSize",
    "timeout",
    "proxyEndpoints",
    SECRETS_KEY
  ]);
  for (const key of Object.keys(value)) {
    if (!allowedTopLevelFields.has(key)) {
      throw new Error(`submission.${key} is not an allowed field; permitted: ${[...allowedTopLevelFields].join(", ")}`);
    }
  }
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
    throw new Error(runtimeSupport.message ?? "unsupported runtime");
  }
  const runtimeSize = parseRuntimeSize(value.runtimeSize);
  const timeoutMs = parseRunTimeout(value.timeout);
  const proxyEndpoints = parseProxyEndpoints(value.proxyEndpoints);
  const secrets = parseInlineSecrets(value.secrets);
  enforceCredentialSecretPolicy(credentialMode, secrets);

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

  const candidate: PlatformRunSubmissionRequest = {
    workspaceId: "",
    idempotencyKey: "",
    credentialMode,
    provider,
    ...(runtime ? { runtime } : {}),
    submission,
    secrets
  };
  const unsupportedManagedFeatures = collectManagedUnsupportedFeatures(candidate);
  if (unsupportedManagedFeatures.length > 0) {
    throw new RuntimeValidationError(
      "feature_runtime_mismatch",
      `The managed runtime does not support these submission features: ` +
        `${unsupportedManagedFeatures.join(", ")}. Remove them or use inline aex skills.`
    );
  }

  return {
    workspaceId: requireString(value.workspaceId, "workspaceId"),
    idempotencyKey: requireString(value.idempotencyKey, "idempotencyKey"),
    credentialMode,
    provider,
    ...(runtime ? { runtime } : {}),
    submission,
    ...(runtimeSize ? { runtimeSize } : {}),
    ...(timeoutMs !== undefined ? { timeoutMs } : {}),
    ...(proxyEndpoints ? { proxyEndpoints } : {}),
    secrets
  };
}

export function parseRuntimeKind(input: unknown): RuntimeKind | undefined {
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

export function parseRunProvider(input: unknown): RunProvider {
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
 * Cross-check the supplied secrets bundle against the credential mode.
 *
 *  - `"byok"`: `secrets.apiKey` (the provider key for the run's `provider`)
 *    MUST be present.
 *  - `"managed"`: `secrets.apiKey` MUST be absent — provider access is
 *    resolved by the managed-key policy, not a caller-supplied key.
 *  - MCP / proxy endpoint auth carry across providers and are not
 *    checked here.
 */
export function enforceCredentialSecretPolicy(
  credentialMode: CredentialMode,
  secrets: PlatformInlineSecrets
): void {
  if (credentialMode === "managed") {
    if (secrets.apiKey !== undefined) {
      throw new Error(
        `secrets.apiKey is not allowed when credentialMode is managed; provider access is resolved by the managed-key policy`
      );
    }
    return;
  }

  if (!secrets.apiKey) {
    throw new Error(`secrets.apiKey is required when credentialMode is byok`);
  }
}

export function parseSubmission(input: unknown): PlatformSubmission {
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
    "outputs",
    "builtins",
    "outputMode",
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
  const outputs = parseOutputs(value.outputs);
  const builtins = parseBuiltins(value.builtins);
  const outputMode = parseOutputMode(value.outputMode);
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
    ...(outputs ? { outputs } : {}),
    ...(builtins !== undefined ? { builtins } : {}),
    ...(outputMode !== undefined ? { outputMode } : {}),
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
        `submission.builtins[${i}] (${JSON.stringify(v)}) is not a valid managed-runtime builtin name; expected /^[a-z][a-z0-9_-]{0,63}$/`
      );
    }
    if (seen.has(v)) continue; // dedupe silently
    seen.add(v);
    out.push(v);
  }
  return out;
}

/**
 * Maximum number of output capture entries accepted per list.
 *
 * 32 is enough room for the typical "one or two capture roots" pattern
 * plus a generous margin for legitimate multi-root use cases (per-tool
 * output directory + scratch state + logs, repeated across a few
 * subdirectories), without inviting abuse of the synthetic-turn path
 * the worker drives at session terminal.
 */
const MAX_OUTPUT_DIRS = 32;

/**
 * Maximum byte length of a single output capture entry (after UTF-8
 * encoding). 512 bytes comfortably covers `/very/long/nested/path`
 * style entries without letting a misuse smuggle large blobs through
 * the field.
 */
const MAX_OUTPUT_DIR_BYTES = 512;

function parseOutputs(input: unknown): PlatformOutputCaptureConfig | undefined {
  if (input === undefined || input === null) {
    return undefined;
  }
  const value = requireRecord(input, "submission.outputs");
  const allowed = new Set(["allowedDirs", "deniedDirs"]);
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) {
      throw new Error(`submission.outputs.${key} is not an allowed field; permitted: ${[...allowed].join(", ")}`);
    }
  }
  const allowedDirs = parseOutputAllowedDirs(value.allowedDirs);
  const deniedDirs = parseOutputDeniedDirs(value.deniedDirs);
  if (!allowedDirs && !deniedDirs) {
    return undefined;
  }
  return {
    ...(allowedDirs ? { allowedDirs } : {}),
    ...(deniedDirs ? { deniedDirs } : {})
  };
}

function parseOutputAllowedDirs(input: unknown): readonly string[] | undefined {
  if (input === undefined) {
    return undefined;
  }
  if (!Array.isArray(input)) {
    throw new Error("submission.outputs.allowedDirs must be an array of absolute UNIX paths");
  }
  if (input.length === 0) {
    // Treat an empty array as omission so the idempotency hash matches
    // the "no allowedDirs" case.
    return undefined;
  }
  if (input.length > MAX_OUTPUT_DIRS) {
    throw new Error(
      `submission.outputs.allowedDirs has ${input.length} entries; max is ${MAX_OUTPUT_DIRS}`
    );
  }
  const seen = new Set<string>();
  const normalised: string[] = [];
  for (let i = 0; i < input.length; i++) {
    const item = input[i];
    if (typeof item !== "string") {
      throw new Error(`submission.outputs.allowedDirs[${i}] must be a string`);
    }
    if (item.length === 0) {
      throw new Error(`submission.outputs.allowedDirs[${i}] must be a non-empty absolute UNIX path`);
    }
    const bytes = new TextEncoder().encode(item).length;
    if (bytes > MAX_OUTPUT_DIR_BYTES) {
      throw new Error(
        `submission.outputs.allowedDirs[${i}] exceeds ${MAX_OUTPUT_DIR_BYTES} bytes (got ${bytes})`
      );
    }
    if (!item.startsWith("/")) {
      throw new Error(
        `submission.outputs.allowedDirs[${i}] must be an absolute UNIX path (start with '/')`
      );
    }
    if (item.includes("\0")) {
      throw new Error(`submission.outputs.allowedDirs[${i}] must not contain NUL bytes`);
    }
    if (item.includes("\n") || item.includes("\r")) {
      throw new Error(`submission.outputs.allowedDirs[${i}] must not contain newline characters`);
    }
    const segments = item.split("/");
    if (segments.includes("..")) {
      throw new Error(`submission.outputs.allowedDirs[${i}] must not contain '..' segments`);
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

function parseOutputDeniedDirs(input: unknown): readonly string[] | undefined {
  if (input === undefined) {
    return undefined;
  }
  if (!Array.isArray(input)) {
    throw new Error("submission.outputs.deniedDirs must be an array of strings");
  }
  if (input.length === 0) {
    return undefined;
  }
  if (input.length > MAX_OUTPUT_DIRS) {
    throw new Error(`submission.outputs.deniedDirs has ${input.length} entries; max is ${MAX_OUTPUT_DIRS}`);
  }
  const seen = new Set<string>();
  const normalised: string[] = [];
  for (let i = 0; i < input.length; i++) {
    const item = input[i];
    if (typeof item !== "string") {
      throw new Error(`submission.outputs.deniedDirs[${i}] must be a string`);
    }
    if (item.length === 0) {
      throw new Error(`submission.outputs.deniedDirs[${i}] must be a non-empty pattern`);
    }
    const bytes = new TextEncoder().encode(item).length;
    if (bytes > MAX_OUTPUT_DIR_BYTES) {
      throw new Error(`submission.outputs.deniedDirs[${i}] exceeds ${MAX_OUTPUT_DIR_BYTES} bytes (got ${bytes})`);
    }
    if (item.includes("\0")) {
      throw new Error(`submission.outputs.deniedDirs[${i}] must not contain NUL bytes`);
    }
    if (item.includes("\n") || item.includes("\r")) {
      throw new Error(`submission.outputs.deniedDirs[${i}] must not contain newline characters`);
    }
    if (item.split("/").includes("..")) {
      throw new Error(`submission.outputs.deniedDirs[${i}] must not contain '..' segments`);
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

function parseSkills(input: unknown): readonly SkillRef[] {
  if (input === undefined) {
    return [];
  }
  if (!Array.isArray(input)) {
    throw new Error("submission.skills must be an array of SkillRef objects");
  }
  const seenProvider = new Set<string>();
  const seenAssetId = new Set<string>();
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
    } else if (ref.kind === "asset") {
      if (seenAssetId.has(ref.assetId)) {
        throw new Error(`submission.skills duplicate assetId: ${ref.assetId}`);
      }
      seenAssetId.add(ref.assetId);
    }
    return ref;
  });
}

function parseAgentsMd(input: unknown): readonly AgentsMdRef[] {
  if (input === undefined) return [];
  if (!Array.isArray(input)) {
    throw new Error("submission.agentsMd must be an array of AgentsMdRef objects");
  }
  const seenAssetId = new Set<string>();
  return input.map((item, index): AgentsMdRef => {
    const path = `submission.agentsMd[${index}]`;
    if (!item || typeof item !== "object" || Array.isArray(item)) {
      throw new Error(`${path} must be an AgentsMdRef object`);
    }
    const raw = item as Record<string, unknown>;
    if (raw.kind !== "asset") {
      throw new Error(`${path}.kind must be 'asset' (got ${JSON.stringify(raw.kind)})`);
    }
    const fields = parseAssetRefFields(raw, path);
    if (seenAssetId.has(fields.assetId)) {
      throw new Error(`submission.agentsMd duplicate assetId: ${fields.assetId}`);
    }
    seenAssetId.add(fields.assetId);
    return { kind: "asset", assetId: fields.assetId, name: fields.name };
  });
}

function parseFiles(input: unknown): readonly FileRef[] {
  if (input === undefined) return [];
  if (!Array.isArray(input)) {
    throw new Error("submission.files must be an array of FileRef objects");
  }
  const seenAssetId = new Set<string>();
  return input.map((item, index): FileRef => {
    const path = `submission.files[${index}]`;
    if (!item || typeof item !== "object" || Array.isArray(item)) {
      throw new Error(`${path} must be a FileRef object`);
    }
    const raw = item as Record<string, unknown>;
    if (raw.kind !== "asset") {
      throw new Error(`${path}.kind must be 'asset' (got ${JSON.stringify(raw.kind)})`);
    }
    const fields = parseAssetRefFields(raw, path);
    if (seenAssetId.has(fields.assetId)) {
      throw new Error(`submission.files duplicate assetId: ${fields.assetId}`);
    }
    seenAssetId.add(fields.assetId);
    if (fields.mountPath !== undefined && !fields.mountPath.startsWith("/")) {
      throw new Error(`${path}.mountPath must start with '/' if provided`);
    }
    return fields.mountPath !== undefined
      ? { kind: "asset", assetId: fields.assetId, name: fields.name, mountPath: fields.mountPath }
      : { kind: "asset", assetId: fields.assetId, name: fields.name };
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
 * Codes emitted when a submission contains features the active runtime cannot
 * serve. Code values are stable so dashboard / SDK error rendering can branch
 * on them.
 */
export const RUNTIME_VALIDATION_CODES = [
  "feature_runtime_mismatch"
] as const;
export type RuntimeValidationCode = (typeof RUNTIME_VALIDATION_CODES)[number];

/**
 * Thrown by `parseRunSubmissionRequest` and `selectRuntime` when the submitted
 * run cannot be served by the active managed runtime. The `code` field is part
 * of the public contract; keep it stable when phrasing changes.
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
 * Walk the parsed submission and collect features that the active managed
 * runtime cannot serve. Provider-hosted skill refs (`Skill.provider(...)`) are
 * rejected now that new submissions only dispatch through managed runs.
 */
export function collectManagedUnsupportedFeatures(req: PlatformRunSubmissionRequest): string[] {
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
 * Backward-incompatible replacement for the old dual-runtime dispatcher. It is
 * kept as a pure helper so SDK, CLI, and tests can resolve the runtime without
 * I/O.
 */
export function selectRuntime(req: PlatformRunSubmissionRequest): RuntimeKind {
  const unsupported = collectManagedUnsupportedFeatures(req);
  if (unsupported.length > 0) {
    throw new RuntimeValidationError(
      "feature_runtime_mismatch",
      `The managed runtime does not support these submission features: ` +
        `${unsupported.join(", ")}. Remove them or use inline aex skills.`
    );
  }
  void req;
  return "managed";
}
