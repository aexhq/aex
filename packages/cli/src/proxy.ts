/**
 * aex proxy — in-container subcommand. Calls an upstream HTTP
 * endpoint via the managed proxy described by the per-run manifest at
 * AEX_INDEX_PATH (`/mnt/session/uploads/aex/index.json`). NO
 * env-var reads; manifest + run token come from fixed paths.
 *
 * This file was extracted from run.ts when the host-side subcommands
 * (run/status/events/outputs/download/cancel/whoami) were added.
 */
import {
  PROXY_PROTOCOL_HEADER,
  PROXY_PROTOCOL_VERSION_V2,
  PROXY_METHOD_HEADER,
  PROXY_PATH_HEADER,
  PROXY_QUERY_HEADER,
  PROXY_HEADERS_HEADER,
  PROXY_RESPONSE_MODE_HEADER,
  PROXY_RESPONSE_MODES,
  PROXY_RESP_MODE_HEADER,
  PROXY_RESP_STATUS_HEADER,
  PROXY_RESP_TRUNCATED_HEADER,
  PROXY_RESP_UPSTREAM_HEADERS_HEADER,
  type ProxyErrorBody,
  type ProxyIndexEntry,
  type ProxyIndexFile,
  type ProxyResponseEnvelope,
  type ProxyResponseMode
} from "@aexhq/contracts";
import { AEX_INDEX_PATH, AEX_RUN_TOKEN_PATH, type CliIO } from "./internal.js";
import { SUCCESS, USAGE_ERR, RUNTIME_ERR, type CliExitCode } from "./host/common.js";

interface ProxyFlags {
  readonly endpointName: string | null;
  readonly method: string;
  readonly path: string;
  readonly query: string | null;
  readonly headers: ReadonlyMap<string, string>;
  readonly dataSpec: string | null;
  readonly responseMode: string | null;
  readonly showHelp: boolean;
}

function parseProxyFlags(rest: readonly string[]): { ok: true; flags: ProxyFlags } | { ok: false; reason: string } {
  let endpointName: string | null = null;
  let method = "GET";
  let path = "/";
  let query: string | null = null;
  const headers = new Map<string, string>();
  let dataSpec: string | null = null;
  let responseMode: string | null = null;
  let showHelp = false;

  for (let i = 0; i < rest.length; i++) {
    const arg = rest[i]!;
    if (arg === "--help" || arg === "-h") {
      showHelp = true;
      continue;
    }
    if (arg === "--method") {
      method = expect(rest, ++i, "--method");
      continue;
    }
    if (arg === "--path") {
      path = expect(rest, ++i, "--path");
      continue;
    }
    if (arg === "--query") {
      query = expect(rest, ++i, "--query");
      continue;
    }
    if (arg === "--header") {
      const kv = expect(rest, ++i, "--header");
      const eq = kv.indexOf("=");
      if (eq <= 0) return { ok: false, reason: "--header must be in the form KEY=VALUE" };
      headers.set(kv.slice(0, eq).toLowerCase(), kv.slice(eq + 1));
      continue;
    }
    if (arg === "--data") {
      dataSpec = expect(rest, ++i, "--data");
      continue;
    }
    if (arg === "--response-mode") {
      responseMode = expect(rest, ++i, "--response-mode");
      continue;
    }
    if (arg.startsWith("--")) {
      return { ok: false, reason: `unknown flag: ${arg}` };
    }
    if (endpointName === null) {
      endpointName = arg;
      continue;
    }
    return { ok: false, reason: `unexpected positional argument: ${arg}` };
  }
  return { ok: true, flags: { endpointName, method, path, query, headers, dataSpec, responseMode, showHelp } };
}

function expect(arr: readonly string[], idx: number, flag: string): string {
  const v = arr[idx];
  if (v === undefined) {
    throw new CliUsageError(`${flag} requires a value`);
  }
  return v;
}

class CliUsageError extends Error {}

export async function printProxyHelp(io: CliIO): Promise<CliExitCode> {
  io.stdout("aex proxy — call an upstream HTTP endpoint via the managed proxy.\n\n");
  io.stdout("Usage:\n");
  io.stdout("  aex proxy <endpoint-name> [flags]\n\n");
  io.stdout("Flags:\n");
  io.stdout("  --method <verb>          HTTP method (default: GET)\n");
  io.stdout("  --path <path>            Caller-supplied path; must match policy prefixes\n");
  io.stdout('  --query <json>           JSON object of query parameters (e.g. \'{"q":"x"}\')\n');
  io.stdout('  --header K=V             Add a caller header (repeatable)\n');
  io.stdout("  --data <value>           Request body. Use '-' for stdin, '@<file>' for file content\n");
  io.stdout("  --response-mode <mode>   status_only | headers_only | full (may only narrow policy)\n");
  io.stdout("  --help                   Show this message\n\n");
  const manifest = await tryReadManifest(io);
  if (manifest && manifest.endpoints.length > 0) {
    io.stdout("Declared endpoints:\n");
    for (const ep of manifest.endpoints) {
      io.stdout(`  • ${formatProxyEndpointSummary(ep)}\n`);
    }
  }
  return SUCCESS;
}

export function formatProxyEndpointSummary(ep: ProxyIndexEntry): string {
  const retry = ep.retry
    ? `, retry=${ep.retry.maxAttempts}x ${ep.retry.retryOnMethods.join("/")} ` +
      `${ep.retry.retryOnStatuses.join("/")} delay=${ep.retry.initialDelayMs}-${ep.retry.maxDelayMs}ms ` +
      `jitter=${ep.retry.jitter}${ep.retry.respectRetryAfter ? " retry-after" : ""}`
    : "";
  return `${ep.name}: ${ep.allowMethods.join(",")} ${ep.allowPathPrefixes.join(",")} (mode=${ep.responseMode}${retry})`;
}

export async function runProxy(io: CliIO, rest: readonly string[]): Promise<CliExitCode> {
  let parsed;
  try {
    parsed = parseProxyFlags(rest);
  } catch (err) {
    if (err instanceof CliUsageError) {
      io.stderr(`${err.message}\n`);
      return USAGE_ERR;
    }
    throw err;
  }
  if (!parsed.ok) {
    io.stderr(`${parsed.reason}\n`);
    return USAGE_ERR;
  }
  const f = parsed.flags;
  if (f.showHelp) {
    return await printProxyHelp(io);
  }
  if (!f.endpointName) {
    io.stderr("missing endpoint-name\n");
    io.stderr("usage: aex proxy <endpoint-name> [flags]\n");
    return USAGE_ERR;
  }
  if (f.responseMode && !(PROXY_RESPONSE_MODES as readonly string[]).includes(f.responseMode)) {
    io.stderr(`--response-mode must be one of: ${PROXY_RESPONSE_MODES.join(", ")}\n`);
    return USAGE_ERR;
  }

  const manifest = await tryReadManifest(io);
  if (!manifest) {
    emitError(io, {
      error: "internal_error",
      message: "manifest not mounted; this CLI must run inside an aex-managed run"
    });
    return RUNTIME_ERR;
  }
  if (!manifest.proxyBaseUrl) {
    emitError(io, {
      error: "endpoint_not_found",
      message: "this run has no proxy endpoints declared",
      endpointName: f.endpointName
    });
    return RUNTIME_ERR;
  }

  let token: string;
  try {
    token = (await io.readFile(AEX_RUN_TOKEN_PATH)).trim();
  } catch {
    emitError(io, {
      error: "unauthorized",
      message: "run token file missing; this run has no proxy bearer"
    });
    return RUNTIME_ERR;
  }
  if (!token) {
    emitError(io, { error: "unauthorized", message: "run token is empty" });
    return RUNTIME_ERR;
  }

  let body: Uint8Array | undefined;
  if (f.dataSpec !== null) {
    try {
      body = await resolveBody(io, f.dataSpec);
    } catch (err) {
      io.stderr(`failed to read request body: ${(err as Error).message}\n`);
      return RUNTIME_ERR;
    }
  }

  const url = `${manifest.proxyBaseUrl.replace(/\/+$/, "")}/${encodeURIComponent(f.endpointName)}`;
  const requestHeaders = new Headers();
  requestHeaders.set("authorization", `Bearer ${token}`);
  requestHeaders.set(PROXY_PROTOCOL_HEADER, PROXY_PROTOCOL_VERSION_V2);
  requestHeaders.set(PROXY_METHOD_HEADER, f.method.toUpperCase());
  requestHeaders.set(PROXY_PATH_HEADER, f.path);
  if (f.query) {
    requestHeaders.set(PROXY_QUERY_HEADER, f.query);
  }
  if (f.headers.size > 0) {
    requestHeaders.set(PROXY_HEADERS_HEADER, JSON.stringify(Object.fromEntries(f.headers)));
  }
  if (f.responseMode) {
    requestHeaders.set(PROXY_RESPONSE_MODE_HEADER, f.responseMode);
  }
  if (body !== undefined) {
    requestHeaders.set("content-length", String(body.byteLength));
  }

  const init: RequestInit = {
    method: "POST",
    headers: requestHeaders,
    redirect: "manual"
  };
  if (body !== undefined) {
    init.body = body;
  }

  let response: Response;
  try {
    response = await io.fetchImpl(url, init);
  } catch (err) {
    emitError(io, {
      error: "upstream_error",
      message: `proxy request failed: ${(err as Error).message}`,
      endpointName: f.endpointName
    });
    return RUNTIME_ERR;
  }

  // v2 streamed success: the hosted API carries the envelope metadata in the
  // x-aex-proxy-* response headers and streams the (already byte-capped)
  // body. Reconstruct the same ProxyResponseEnvelope JSON the agent saw
  // under v1 so the stdout contract is unchanged. A BFF-level error (e.g.
  // unsupported_protocol, policy_denied) still returns a JSON error body
  // with no status header, so it falls through to the JSON path below.
  if (response.headers.get(PROXY_RESP_STATUS_HEADER) !== null) {
    const envelope = await readStreamedEnvelope(response, f.endpointName);
    io.stdout(JSON.stringify(envelope) + "\n");
    return SUCCESS;
  }

  const text = await response.text();
  let parsedBody: unknown;
  try {
    parsedBody = text ? JSON.parse(text) : {};
  } catch {
    // Include HTTP status + a bounded body snippet so the agent's
    // tool_result log surfaces what actually came back. Without this,
    // a non-JSON response (e.g. a 500 HTML error page from the BFF
    // host) is indistinguishable from any other failure.
    const snippet = text.slice(0, 200).replace(/\s+/g, " ").trim();
    emitError(io, {
      error: "internal_error",
      message: `proxy returned non-JSON response (HTTP ${response.status}, body=${JSON.stringify(snippet)})`,
      endpointName: f.endpointName
    });
    return RUNTIME_ERR;
  }

  if (!response.ok) {
    io.stderr(JSON.stringify(parsedBody) + "\n");
    return RUNTIME_ERR;
  }

  io.stdout(JSON.stringify(parsedBody) + "\n");
  return SUCCESS;
}

async function resolveBody(io: CliIO, spec: string): Promise<Uint8Array> {
  if (spec === "-") {
    const data = await io.readFile("/dev/stdin");
    return new Uint8Array(Buffer.from(data, "utf8"));
  }
  if (spec.startsWith("@")) {
    const path = spec.slice(1);
    if (!path) throw new Error("--data @<file> requires a path");
    const data = await io.readFile(path);
    return new Uint8Array(Buffer.from(data, "utf8"));
  }
  return new Uint8Array(Buffer.from(spec, "utf8"));
}

function emitError(io: CliIO, body: ProxyErrorBody): void {
  io.stderr(JSON.stringify(body) + "\n");
}

/**
 * Reassemble a {@link ProxyResponseEnvelope} from a v2 streamed response.
 * The hosted API has already enforced the byte-cap, so reading the body to the
 * end here is bounded by `maxResponseBytes` — the OOM risk lived on the
 * API process, not in this per-call container process.
 */
async function readStreamedEnvelope(
  response: Response,
  endpointName: string
): Promise<ProxyResponseEnvelope> {
  const effectiveResponseMode = (response.headers.get(PROXY_RESP_MODE_HEADER) ??
    "headers_only") as ProxyResponseMode;
  const upstreamStatus = Number.parseInt(response.headers.get(PROXY_RESP_STATUS_HEADER) ?? "0", 10);
  let upstreamHeaders: Record<string, string> = {};
  const rawHeaders = response.headers.get(PROXY_RESP_UPSTREAM_HEADERS_HEADER);
  if (rawHeaders) {
    try {
      const parsed = JSON.parse(rawHeaders);
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
        upstreamHeaders = parsed as Record<string, string>;
      }
    } catch {
      // The metadata header is controlled by the hosted API; a malformed value is a
      // protocol bug, not agent input. Degrade to empty rather than crash.
    }
  }
  const truncatedRaw = response.headers.get(PROXY_RESP_TRUNCATED_HEADER);
  const bytes = new Uint8Array(await response.arrayBuffer());

  return {
    endpointName,
    upstreamStatus,
    upstreamHeaders,
    effectiveResponseMode,
    // The streamed path can't echo the request-side clamp decision (it
    // isn't carried back); the effective mode is authoritative for the
    // agent and matches what v1 surfaced for an un-clamped call.
    modeClamped: false,
    ...(effectiveResponseMode === "full" && bytes.byteLength > 0
      ? { upstreamBodyBase64: Buffer.from(bytes).toString("base64") }
      : {}),
    ...(truncatedRaw === "true" ? { truncated: true } : {})
  };
}

export async function tryReadManifest(io: CliIO): Promise<ProxyIndexFile | null> {
  try {
    const raw = await io.readFile(AEX_INDEX_PATH);
    return JSON.parse(raw) as ProxyIndexFile;
  } catch {
    return null;
  }
}
