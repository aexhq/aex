/**
 * Shared helpers for every host-side aex subcommand. Common flag
 * parsing, HttpClient construction, manifest detection so we can refuse
 * to run host commands inside a managed session container, and exit codes.
 */
import {
  AEX_DEFAULT_BASE_URL,
  AexApiError,
  AexError,
  AexNetworkError,
  HttpClient,
  extractErrorCode,
  redactSecrets,
  type SessionStatus
} from "@aexhq/contracts";
import { AEX_INDEX_PATH, type CliIO } from "../internal.js";

// One shared "did you mean?" suggester (contracts SSoT), used by the SDK's
// unknown-model resolver AND the CLI's near-miss --provider/--runtime-size
// hints — no CLI-local duplicate to drift from the SDK.
export { suggest } from "@aexhq/contracts";

export interface CliExitCode {
  readonly code: number;
}

export const SUCCESS: CliExitCode = { code: 0 };
export const USAGE_ERR: CliExitCode = { code: 2 };
export const RUNTIME_ERR: CliExitCode = { code: 1 };
/** Conventional shell exit status for a process interrupted by SIGINT. */
export const INTERRUPTED_ERR: CliExitCode = { code: 130 };
/**
 * Distinct exit code for "the wait/follow deadline elapsed before the
 * run reached a terminal status". Separated from RUNTIME_ERR (1) so a
 * script can tell a timeout apart from a session that finished non-succeeded
 * (which is also RUNTIME_ERR). Mirrors the SDK's `waitForRun` throwing a
 * dedicated timeout error.
 */
export const TIMEOUT_ERR: CliExitCode = { code: 3 };

const NON_PROGRESSING_SESSION_STATUSES = new Set<SessionStatus>([
  "idle",
  "suspended",
  "awaiting_approval",
  "error",
  "deleted",
  "expired"
]);

/**
 * A session is not progressing once its thread is idle, held, recoverably
 * errored, or removed. RUN event terminals remain the authoritative run boundary.
 */
export function isSessionNonProgressing(status: string): boolean {
  return NON_PROGRESSING_SESSION_STATUSES.has(status as SessionStatus);
}

export interface CommonHostFlags {
  readonly apiKey: string;
  readonly aexUrl: string;
  /** `--debug`: print a redacted per-request trace to stderr. Uploads nothing. */
  readonly debug: boolean;
  /**
   * `--json`: a globally-recognized output flag no verb rejects. Most verbs
   * already emit JSON, so it is a no-op there; the render-toggling verbs
   * (`tail`/`inspect`/`billing`) read it to switch human ↔ machine output.
   * Recognized centrally so `aex whoami --json` (and any verb) never fails
   * with "unexpected arguments".
   */
  readonly json: boolean;
}

export type ParseCommonResult =
  | { readonly ok: true; readonly flags: CommonHostFlags; readonly rest: readonly string[] }
  | { readonly ok: false; readonly reason: string };

/** Auth policy for an authenticated host command. Keep the planes explicit. */
export type HostAuthPolicy = "data" | "control";

/** Raw, pre-resolution extraction: token/url may be `null` (no required check). */
export interface ExtractedCommonHostFlags {
  readonly apiKey: string | null;
  readonly aexUrl: string | null;
  readonly debug: boolean;
  readonly json: boolean;
  readonly rest: readonly string[];
}

export type ExtractCommonResult =
  | { readonly ok: true; readonly flags: ExtractedCommonHostFlags }
  | { readonly ok: false; readonly reason: string };

export interface ExtractedGlobalFlags {
  readonly json: boolean;
  readonly rest: readonly string[];
}

/**
 * Pure extraction for flags that are global after the top-level verb.
 * Exact `--json` tokens are recognized in every position and every occurrence;
 * all other tokens retain their order for the owning command parser.
 */
export function extractGlobalFlags(argv: readonly string[]): ExtractedGlobalFlags {
  const rest: string[] = [];
  let json = false;
  for (const arg of argv) {
    if (arg === "--json") {
      json = true;
      continue;
    }
    rest.push(arg);
  }
  return { json, rest };
}

/**
 * Pure, synchronous extraction of the common host flags from argv. It leaves
 * `apiKey`/`aexUrl` as `null` when absent so the live resolvers can apply their
 * respective stored-config policies. Touches no IO and reads no env.
 *
 * There is no `--workspace` flag: workspace identity is derived server-side
 * from the API key.
 */
export function extractCommonHostFlags(argv: readonly string[]): ExtractCommonResult {
  let apiKey: string | null = null;
  let aexUrl: string | null = null;
  let debug = false;
  const rest: string[] = [];

  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i]!;
    if (arg === "--debug") {
      debug = true;
      continue;
    }
    if (arg === "--api-key") {
      const v = argv[++i];
      if (v === undefined) return { ok: false, reason: "--api-key requires a value" };
      apiKey = v;
      continue;
    }
    if (arg.startsWith("--api-key=")) {
      apiKey = arg.slice("--api-key=".length);
      continue;
    }
    if (arg === "--aex-url") {
      const v = argv[++i];
      if (v === undefined) return { ok: false, reason: "--aex-url requires a value" };
      aexUrl = v;
      continue;
    }
    if (arg.startsWith("--aex-url=")) {
      aexUrl = arg.slice("--aex-url=".length);
      continue;
    }
    if (arg === "--workspace" || arg === "--workspace-id") {
      // Removed in favor of server-side derivation from the API key.
      // Bail out early with an actionable message instead of letting
      // the flag fall through and produce a confusing positional-arg
      // count error from the calling subcommand.
      return {
        ok: false,
        reason: `unknown flag ${arg}: workspace is derived from --api-key on the server; drop this flag`
      };
    }
    rest.push(arg);
  }

  const global = extractGlobalFlags(rest);
  return { ok: true, flags: { apiKey, aexUrl, debug, json: global.json, rest: global.rest } };
}

export function rejectUnknownFlags(io: CliIO, rest: readonly string[], usage: string): CliExitCode | null {
  const unknown = rest.find((arg) => arg.startsWith("--"));
  if (unknown === undefined) return null;
  io.stderr(`unknown flag: ${unknown}\n`);
  io.stderr(`${usage}\n`);
  return USAGE_ERR;
}

/**
 * Resolve the common host flags with the stored-config fallback (DX1).
 * Precedence: `--api-key` flag > stored token; `--aex-url` flag > stored url >
 * default. When neither a flag nor a stored token supplies the bearer, returns
 * an actionable `ok:false` pointing at `aex login`.
 *
 * Under `--debug` it emits ONE non-secret auth-source line to stderr (which
 * source won + the resolved url) so "wrong plane / wrong token" is a one-glance
 * diagnosis. The token value is NEVER logged.
 */
export async function resolveCommonHostFlags(
  io: CliIO,
  argv: readonly string[]
): Promise<ParseCommonResult> {
  const resolved = await resolveHostAuth(io, argv, "data");
  if (!resolved.ok) return resolved;
  const { auth: _auth, ...compatible } = resolved;
  return compatible;
}

/** Which credential a control-plane resolution selected. */
export type ControlPlaneAuthSource = "flag" | "account" | "workspace";

export type ResolveControlPlaneResult =
  | {
      readonly ok: true;
      readonly flags: CommonHostFlags;
      readonly rest: readonly string[];
      readonly source: ControlPlaneAuthSource;
      readonly defaultOrgId?: string;
      readonly defaultWorkspaceId?: string;
    }
  | { readonly ok: false; readonly reason: string };

/**
 * Resolve the bearer for a CONTROL-PLANE verb (`orgs`/`workspaces`/`keys`).
 * Where {@link resolveCommonHostFlags} prefers a workspace (data-plane) key,
 * this prefers the ACCOUNT token persisted by the `aex login` device flow, so a
 * control-plane command reaches the account principal that spans your orgs.
 *
 * Precedence: `--api-key` flag (an explicitly supplied PAT) > stored
 * `accountToken` > stored workspace `apiKey` (last-resort — the server rejects a
 * workspace key for control-plane, but this yields an actionable 401/403 rather
 * than a confusing "no credential" before the request). An account PAT is NOT
 * self-describing, so the base URL comes from `--aex-url` > stored `aexUrl` >
 * the prd default (never key-derived).
 */
export async function resolveControlPlaneHostFlags(
  io: CliIO,
  argv: readonly string[]
): Promise<ResolveControlPlaneResult> {
  const resolved = await resolveHostAuth(io, argv, "control");
  if (!resolved.ok) return resolved;
  const { auth: _auth, ...compatible } = resolved;
  return compatible;
}

type ResolvedDataHostAuth = {
  readonly ok: true;
  readonly auth: "data";
  readonly flags: CommonHostFlags;
  readonly rest: readonly string[];
};

type ResolvedControlHostAuth = {
  readonly ok: true;
  readonly auth: "control";
  readonly flags: CommonHostFlags;
  readonly rest: readonly string[];
  readonly source: ControlPlaneAuthSource;
  readonly defaultOrgId?: string;
  readonly defaultWorkspaceId?: string;
};

type ResolveHostAuthFailure = { readonly ok: false; readonly reason: string };

async function resolveHostAuth(
  io: CliIO,
  argv: readonly string[],
  auth: "data"
): Promise<ResolvedDataHostAuth | ResolveHostAuthFailure>;
async function resolveHostAuth(
  io: CliIO,
  argv: readonly string[],
  auth: "control"
): Promise<ResolvedControlHostAuth | ResolveHostAuthFailure>;
async function resolveHostAuth(
  io: CliIO,
  argv: readonly string[],
  auth: HostAuthPolicy
): Promise<ResolvedDataHostAuth | ResolvedControlHostAuth | ResolveHostAuthFailure>;
async function resolveHostAuth(
  io: CliIO,
  argv: readonly string[],
  auth: HostAuthPolicy
): Promise<ResolvedDataHostAuth | ResolvedControlHostAuth | ResolveHostAuthFailure> {
  const extracted = extractCommonHostFlags(argv);
  if (!extracted.ok) return extracted;
  const { apiKey: flagToken, aexUrl: flagUrl, debug, json, rest } = extracted.flags;

  if (auth === "data") {
    let token = flagToken;
    let url = flagUrl;
    let source: "flag" | "stored" | "none" = flagToken ? "flag" : "none";
    let storedLocation = "";

    if (!token && io.configStore) {
      const stored = await io.configStore.read();
      storedLocation = io.configStore.location();
      if (stored?.apiKey) {
        token = stored.apiKey;
        source = "stored";
        if (!url && stored.aexUrl) url = stored.aexUrl;
      }
    }

    const resolvedUrl = url ?? AEX_DEFAULT_BASE_URL;
    if (debug) {
      const where =
        source === "flag"
          ? "--api-key flag"
          : source === "stored"
            ? `stored token (${storedLocation})`
            : "none";
      io.stderr(`[aex] auth: ${where}; aex-url=${resolvedUrl}\n`);
    }
    if (!token) return { ok: false, reason: "no API key — pass --api-key or run `aex login`" };
    return { ok: true, auth, flags: { apiKey: token, aexUrl: resolvedUrl, debug, json }, rest };
  }

  let token = flagToken;
  let url = flagUrl;
  let source: ControlPlaneAuthSource | "none" = flagToken ? "flag" : "none";
  let storedLocation = "";
  let defaultOrgId: string | undefined;
  let defaultWorkspaceId: string | undefined;

  if (io.configStore) {
    const stored = await io.configStore.read();
    storedLocation = io.configStore.location();
    defaultOrgId = stored?.defaultOrgId;
    defaultWorkspaceId = stored?.defaultWorkspaceId;
    if (!token && stored?.accountToken) {
      token = stored.accountToken;
      source = "account";
    } else if (!token && stored?.apiKey) {
      token = stored.apiKey;
      source = "workspace";
    }
    if (!url && stored?.aexUrl) url = stored.aexUrl;
  }

  const resolvedUrl = url ?? AEX_DEFAULT_BASE_URL;

  if (debug) {
    const where =
      source === "flag"
        ? "--api-key flag"
        : source === "account"
          ? `stored account token (${storedLocation})`
          : source === "workspace"
            ? `stored workspace key (${storedLocation})`
            : "none";
    io.stderr(`[aex] control-plane auth: ${where}; aex-url=${resolvedUrl}\n`);
  }

  if (!token || source === "none") {
    return {
      ok: false,
      reason: "no account credential — run `aex login` (device flow) or pass an account PAT via --api-key"
    };
  }
  return {
    ok: true,
    auth,
    flags: { apiKey: token, aexUrl: resolvedUrl, debug, json },
    rest,
    source,
    ...(defaultOrgId ? { defaultOrgId } : {}),
    ...(defaultWorkspaceId ? { defaultWorkspaceId } : {})
  };
}

export interface PrepareHostCommandOptions<Auth extends HostAuthPolicy> {
  readonly verb: string;
  readonly auth: Auth;
  /** Command-owned presentation for an otherwise shared preparation failure. */
  readonly formatResolutionError?: (reason: string) => string;
}

export type PrepareHostCommandFailure = {
  readonly ok: false;
  readonly exit: CliExitCode;
};

/**
 * Prepare one authenticated host command before its business parsing begins.
 * Refusal runs first and fails closed; credential resolution then follows the
 * explicit data/control policy. Neither step performs a network request.
 */
export function prepareHostCommand(
  io: CliIO,
  argv: readonly string[],
  options: PrepareHostCommandOptions<"data">
): Promise<ResolvedDataHostAuth | PrepareHostCommandFailure>;
export function prepareHostCommand(
  io: CliIO,
  argv: readonly string[],
  options: PrepareHostCommandOptions<"control">
): Promise<ResolvedControlHostAuth | PrepareHostCommandFailure>;
export function prepareHostCommand(
  io: CliIO,
  argv: readonly string[],
  options: PrepareHostCommandOptions<HostAuthPolicy>
): Promise<ResolvedDataHostAuth | ResolvedControlHostAuth | PrepareHostCommandFailure>;
export async function prepareHostCommand(
  io: CliIO,
  argv: readonly string[],
  options: PrepareHostCommandOptions<HostAuthPolicy>
): Promise<ResolvedDataHostAuth | ResolvedControlHostAuth | PrepareHostCommandFailure> {
  if (await refuseInsideManagedSession(io, options.verb)) {
    return { ok: false, exit: USAGE_ERR };
  }
  const resolved = await resolveHostAuth(io, argv, options.auth);
  if (!resolved.ok) {
    const message = options.formatResolutionError?.(resolved.reason) ?? resolved.reason;
    io.stderr(`${message}\n`);
    return { ok: false, exit: USAGE_ERR };
  }
  return resolved;
}

/**
 * Map a thrown SDK/transport error to a structured, actionable shape for the
 * CLI's JSON error envelope. Pulls `status` from {@link AexApiError}, a stable
 * `code` from {@link AexError}, and attaches a one-line `remedy` keyed on the
 * HTTP status so a failure tells the operator what to do next. Secret-free
 * (the error's own message is already redaction-scanned by `AexError`).
 */
export function describeApiError(err: unknown): {
  readonly code: string;
  readonly message: string;
  readonly status?: number;
  readonly remedy?: string;
} {
  if (err instanceof AexApiError) {
    const remedy = remedyForStatus(err.status);
    const detail = describeErrorBody(err.body);
    return {
      code: err.code,
      message: detail ? `${err.message} — ${detail}` : err.message,
      status: err.status,
      ...(remedy ? { remedy } : {})
    };
  }
  if (err instanceof AexError) {
    // Surface the transport failure code (ECONNREFUSED/ENOTFOUND/…) —
    // undici hides it on `cause` — plus a connectivity remedy keyed on it.
    const causeCode = err instanceof AexNetworkError ? err.causeCode : extractErrorCode(err.cause);
    const remedy = causeCode ? remedyForNetworkCode(causeCode) : undefined;
    const message = causeCode && !err.message.includes(causeCode) ? `${err.message} (${causeCode})` : err.message;
    return { code: err.code, message, ...(remedy ? { remedy } : {}) };
  }
  return { code: "error", message: err instanceof Error ? err.message : String(err) };
}

/**
 * Standard aex error-envelope keys. A body carrying ONLY these adds nothing
 * beyond the message `describeApiError` already extracted, so it is omitted.
 * The extra-detail append renders only the NON-standard keys (`requestId`,
 * `requiredScope`, …), so the message never repeats the `error`/`message`
 * the caller already sees.
 */
const STANDARD_ERROR_BODY_KEYS = new Set(["ok", "error", "message", "code"]);

/**
 * Render an `AexApiError.body` for the CLI envelope when it carries fields
 * beyond the standard `{ ok, error, code, message }` shape — and
 * render ONLY those extra fields (e.g. `requiredScope`, `status`), never the
 * standard ones the message already carries. Truncated and redaction-scanned
 * (the body is already redacted at construction; this is a cheap second pass).
 */
function describeErrorBody(body: unknown): string | undefined {
  if (!body || typeof body !== "object") return undefined;
  const record = body as Record<string, unknown>;
  const extraKeys = Object.keys(record).filter((key) => !STANDARD_ERROR_BODY_KEYS.has(key));
  if (extraKeys.length === 0) return undefined;
  let text: string;
  try {
    text = JSON.stringify(Object.fromEntries(extraKeys.map((key) => [key, record[key]])));
  } catch {
    return undefined;
  }
  const redacted = redactSecrets(text);
  return redacted.length > 400 ? `${redacted.slice(0, 400)}… (truncated)` : redacted;
}

function remedyForStatus(status: number): string | undefined {
  // A garbled/truncated token surfaces as 400 malformed_token (not 401), so the
  // most common credential mistake needs the same "check the token" nudge (F17).
  if (status === 400) return "malformed request — if this is an auth failure, check --api-key or run `aex login`";
  if (status === 401) return "check --api-key, or run `aex login`";
  if (status === 403) return "token lacks permission for this workspace/action";
  if (status === 404) return "no such run/resource — verify the id";
  if (status === 429) return "rate limited — retry with backoff";
  if (status >= 500) return "server error — retry; rerun with --debug to capture the request trace";
  return undefined;
}

/** Network sibling of {@link remedyForStatus}, keyed on the transport code. */
function remedyForNetworkCode(code: string): string | undefined {
  switch (code) {
    case "ECONNREFUSED":
    case "ENOTFOUND":
    case "EAI_AGAIN":
    case "EHOSTUNREACH":
    case "ENETUNREACH":
      return "cannot reach the aex API — check --aex-url and network connectivity";
    case "ECONNRESET":
    case "ETIMEDOUT":
    case "UND_ERR_CONNECT_TIMEOUT":
      return "connection dropped — retry; check --aex-url, network connectivity, or any proxy/VPN";
    case "CERT_HAS_EXPIRED":
    case "DEPTH_ZERO_SELF_SIGNED_CERT":
    case "UNABLE_TO_VERIFY_LEAF_SIGNATURE":
      return "TLS verification failed — check --aex-url points at the right host";
    default:
      return undefined;
  }
}

export function makeHttpClient(io: CliIO, flags: CommonHostFlags): HttpClient {
  return new HttpClient({
    baseUrl: flags.aexUrl,
    apiKey: flags.apiKey,
    retryTransientGets: true,
    fetch: io.fetchImpl,
    // `--debug`: route the transport's redacted per-request traces to stderr.
    ...(flags.debug ? { debug: (line: string) => io.stderr(`${line}\n`) } : {})
  });
}

/**
 * Host subcommands refuse to execute inside a managed session container. The
 * heuristic: presence of the per-session manifest at AEX_INDEX_PATH
 * (`/mnt/session/uploads/aex/index.json`) means we're inside a
 * session, not on a developer host.
 *
 * Fails *closed* on read errors that are not "file not found": if the
 * manifest exists but is unreadable for any other reason (permissions,
 * IO error, etc.) we refuse to execute the host verb rather than risk
 * leaking workspace-management calls into a sandboxed container.
 */
export async function refuseInsideManagedSession(io: CliIO, verb: string): Promise<boolean> {
  try {
    await io.readFile(AEX_INDEX_PATH);
    io.stderr(
      `\`aex ${verb}\` is a host command and cannot execute inside a managed session container.\n` +
      "Make HTTP calls from your code and pass credentials through secrets.\n"
    );
    return true;
  } catch (err) {
    const code = (err as NodeJS.ErrnoException | undefined)?.code;
    if (code === "ENOENT" || code === "ENOTDIR") {
      // Manifest definitively absent; we are on a host. Allow the verb.
      return false;
    }
    io.stderr(
      `\`aex ${verb}\` could not determine whether it is running inside a managed session ` +
      `container (error reading ${AEX_INDEX_PATH}: ${(err as Error).message ?? "unknown"}). ` +
      `Refusing to proceed.\n`
    );
    return true;
  }
}

/**
 * Emit a JSON error body and return RUNTIME_ERR.
 */
export function emitJsonError(io: CliIO, code: string, message: string, extra: Record<string, unknown> = {}): CliExitCode {
  io.stderr(JSON.stringify({ error: code, message, ...extra }) + "\n");
  return RUNTIME_ERR;
}

/** Command-owned context accepted by {@link emitApiError}. */
export type ApiErrorDetails = Readonly<Record<string, unknown>> & {
  /** Owned by the emitter's positional `code` argument. */
  readonly error?: never;
  /** Owned by {@link describeApiError}. */
  readonly message?: never;
  /** Derived from {@link describeApiError}. */
  readonly status?: never;
  /** Derived from {@link describeApiError}. */
  readonly remedy?: never;
};

/** Optional presentation applied after the thrown error is safely described. */
export interface ApiErrorEmissionOptions {
  readonly messagePrefix?: string;
}

/**
 * Describe an SDK/API failure and emit its command-specific CLI JSON envelope.
 * Command details retain their insertion order before the optional centrally
 * derived `status` and `remedy` fields.
 */
export function emitApiError(
  io: CliIO,
  code: string,
  err: unknown,
  details: ApiErrorDetails = {},
  options: ApiErrorEmissionOptions = {}
): CliExitCode {
  const described = describeApiError(err);
  const message = options.messagePrefix
    ? `${options.messagePrefix}${described.message}`
    : described.message;
  return emitJsonError(io, code, message, {
    ...details,
    ...(described.status !== undefined ? { status: described.status } : {}),
    ...(described.remedy ? { remedy: described.remedy } : {})
  });
}

/**
 * Repeatable `--var key=value` / `--mcp-secret name=value` /
 * `--name=value`-style flag parser. Returns the values in
 * insertion order (last-wins on duplicate keys).
 */
export function collectRepeatedKv(rest: readonly string[], flag: string): {
  readonly entries: Record<string, string>;
  readonly remaining: readonly string[];
  readonly error: string | null;
} {
  const entries: Record<string, string> = {};
  const remaining: string[] = [];
  const prefix = `${flag}=`;
  for (let i = 0; i < rest.length; i++) {
    const arg = rest[i]!;
    let kv: string;
    if (arg === flag) {
      const next = rest[++i];
      if (next === undefined) {
        return { entries, remaining, error: `${flag} requires a KEY=VALUE argument` };
      }
      kv = next;
    } else if (arg.startsWith(prefix)) {
      kv = arg.slice(prefix.length);
    } else {
      remaining.push(arg);
      continue;
    }
    const eq = kv.indexOf("=");
    if (eq <= 0) {
      return { entries, remaining, error: `${flag} must be in the form KEY=VALUE (got: ${kv})` };
    }
    entries[kv.slice(0, eq)] = kv.slice(eq + 1);
  }
  return { entries, remaining, error: null };
}

/**
 * Repeatable `--flag <value>` / `--flag=<value>` collector. Returns the
 * collected values in insertion order alongside the remaining argv.
 */
export function collectRepeated(rest: readonly string[], flag: string): {
  readonly values: readonly string[];
  readonly remaining: readonly string[];
  readonly error: string | null;
} {
  const values: string[] = [];
  const remaining: string[] = [];
  const prefix = `${flag}=`;
  for (let i = 0; i < rest.length; i++) {
    const arg = rest[i]!;
    if (arg === flag) {
      const v = rest[++i];
      if (v === undefined) {
        return { values, remaining, error: `${flag} requires a value` };
      }
      values.push(v);
      continue;
    }
    if (arg.startsWith(prefix)) {
      values.push(arg.slice(prefix.length));
      continue;
    }
    remaining.push(arg);
  }
  return { values, remaining, error: null };
}

/**
 * Repeatable `--flag KEY=VALUE` collector that PRESERVES every
 * occurrence (no key-collision collapse). Use this when the same key
 * may legitimately appear multiple times — for example,
 * `--mcp-auth github=Authorization:Bearer t --mcp-auth github=X-Api-Key:k`
 * needs to register both headers, not just the last one.
 *
 * Returns `[key, value]` pairs in argv order alongside the remaining
 * argv with the consumed flags removed.
 */
export function collectRepeatedKvList(rest: readonly string[], flag: string): {
  readonly entries: ReadonlyArray<readonly [string, string]>;
  readonly remaining: readonly string[];
  readonly error: string | null;
} {
  const entries: Array<readonly [string, string]> = [];
  const remaining: string[] = [];
  const prefix = `${flag}=`;
  for (let i = 0; i < rest.length; i++) {
    const arg = rest[i]!;
    let kv: string;
    if (arg === flag) {
      const next = rest[++i];
      if (next === undefined) {
        return { entries, remaining, error: `${flag} requires a KEY=VALUE argument` };
      }
      kv = next;
    } else if (arg.startsWith(prefix)) {
      kv = arg.slice(prefix.length);
    } else {
      remaining.push(arg);
      continue;
    }
    const eq = kv.indexOf("=");
    if (eq <= 0) {
      return { entries, remaining, error: `${flag} must be in the form KEY=VALUE (got: ${kv})` };
    }
    entries.push([kv.slice(0, eq), kv.slice(eq + 1)] as const);
  }
  return { entries, remaining, error: null };
}

/**
 * Parse a human-friendly duration into milliseconds. Accepts a bare
 * integer (interpreted as milliseconds) or `<number><unit>` where unit
 * is one of `ms`, `s`, `m`, `h` — e.g. `500ms`, `30s`, `8m`, `1h`.
 *
 * Returns the millisecond count, or an `error` string describing the
 * malformed input (the value is never silently coerced to 0). Rejects
 * negatives and non-finite values so a `--timeout` can never disable the
 * deadline by accident.
 */
export function parseDuration(input: string): { readonly ms: number | null; readonly error: string | null } {
  const match = /^(\d+(?:\.\d+)?)(ms|s|m|h)?$/.exec(input.trim());
  if (!match) {
    return { ms: null, error: `invalid duration "${input}" (expected e.g. 500ms, 30s, 8m, 1h, or a bare ms integer)` };
  }
  const value = Number(match[1]);
  if (!Number.isFinite(value) || value < 0) {
    return { ms: null, error: `invalid duration "${input}" (must be a non-negative number)` };
  }
  const unit = match[2] ?? "ms";
  const factor = unit === "h" ? 3_600_000 : unit === "m" ? 60_000 : unit === "s" ? 1_000 : 1;
  return { ms: Math.round(value * factor), error: null };
}

export function takeBooleanFlag(rest: readonly string[], flag: string): {
  readonly present: boolean;
  readonly remaining: readonly string[];
} {
  const remaining: string[] = [];
  let present = false;
  for (const arg of rest) {
    if (arg === flag) {
      present = true;
      continue;
    }
    remaining.push(arg);
  }
  return { present, remaining };
}

/**
 * Take an option flag in either `--flag value` or `--flag=value` form.
 * Returns the trailing value (or undefined when the flag is absent), the
 * remaining args, and an explicit error when a present flag has no value.
 */
export function takeOptionFlag(
  rest: readonly string[],
  flag: string
): { readonly value: string | undefined; readonly remaining: readonly string[]; readonly error: string | null } {
  const remaining: string[] = [];
  let value: string | undefined;
  const prefix = `${flag}=`;
  for (let i = 0; i < rest.length; i++) {
    const arg = rest[i]!;
    if (arg === flag) {
      const next = rest[++i];
      if (next === undefined) return { value, remaining, error: `${flag} requires a value` };
      value = next;
      continue;
    }
    if (arg.startsWith(prefix)) {
      value = arg.slice(prefix.length);
      continue;
    }
    remaining.push(arg);
  }
  return { value, remaining, error: null };
}
