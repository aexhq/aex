// Wire format between the in-container runtime bridge (mounted at
// `/mnt/session/uploads/aex/aex`, invoked through `bun`) and
// the hosted API's named proxy route
// (`POST /api/runs/:runId/proxy/:endpointName`).
//
// This module is the single source of truth for the request shape, the
// response shape, the error-code enum, and the protocol version header.
// CLI and BFF both import from here; drift becomes a build-time type error.
//
/**
 * Wire-protocol version. Bumped on any breaking change to the request or
 * response shape. The CLI sends this in the `X-Aex-Proxy-Protocol`
 * header on every request; the BFF rejects mismatches with HTTP 426
 * `unsupported_protocol`.
 *
 * Bumps are coordinated: CLI and BFF release together, the hosted API
 * bundles the matching CLI artifact, and the e2e suite runs both with
 * the new version.
 */
export const PROXY_PROTOCOL_VERSION = "1" as const;

/**
 * Streaming named-proxy protocol. A client that sends `"2"` in
 * {@link PROXY_PROTOCOL_HEADER} opts into the streamed response path: the
 * hosted API pipes the upstream body back unbuffered (no base64, no
 * full-body JSON envelope) and carries the envelope metadata
 * in the `x-aex-proxy-*` response headers below instead of a JSON envelope.
 *
 * Additive: `"1"` stays valid and keeps the buffered
 * {@link ProxyResponseEnvelope}. Old runners keep working; new runners
 * stream. The version is on the request header so the hosted API can serve
 * both shapes without a coordinated CLI/BFF release.
 */
export const PROXY_PROTOCOL_VERSION_V2 = "2" as const;

export const PROXY_PROTOCOL_HEADER = "x-aex-proxy-protocol";

/**
 * Response headers for the streamed (v2) named-proxy path. The hosted API sets
 * these BEFORE it starts streaming the upstream body, so the client can
 * reconstruct the same fields the v1 {@link ProxyResponseEnvelope} carried
 * without buffering. All values are plain strings; numeric fields are
 * decimal, booleans are `"true"`/`"false"`.
 */
export const PROXY_RESP_STATUS_HEADER = "x-aex-proxy-status";
export const PROXY_RESP_MODE_HEADER = "x-aex-proxy-effective-mode";
/**
 * `"true"` when the cap forced truncation, `"false"` when the full body
 * fit, `"unknown"` when the upstream omitted `content-length` and the body
 * happened to reach the cap (can't distinguish exact-fit from over-cap
 * without buffering). See the streaming byte-cap note in proxy-routes.ts.
 */
export const PROXY_RESP_TRUNCATED_HEADER = "x-aex-proxy-truncated";
/** JSON object of lowercase upstream header names → values (mode-filtered). */
export const PROXY_RESP_UPSTREAM_HEADERS_HEADER = "x-aex-proxy-upstream-headers";

/**
 * Default `User-Agent` the proxy attaches to every outbound request when
 * the caller did not supply one via `allowHeaders`. Some upstreams reject
 * requests that arrive without a meaningful UA — notably the Wikimedia
 * family (Wikidata, Wikipedia, Wikimedia Commons), whose policy requires
 * a contactable identifier and otherwise returns HTTP 403 with a
 * `Please identify your user agent` body.
 *
 * Callers can override per request by listing `user-agent` in their
 * endpoint's `allowHeaders` and setting it on the proxy call; the
 * default only fires when nothing was forwarded.
 *
 * See <https://meta.wikimedia.org/wiki/User-Agent_policy>.
 */
export const PROXY_DEFAULT_USER_AGENT = "aex-proxy/1.0 (+https://aex.dev/contact)";

export const PROXY_METHOD_HEADER = "x-aex-method";
export const PROXY_PATH_HEADER = "x-aex-path";
export const PROXY_QUERY_HEADER = "x-aex-query";
export const PROXY_HEADERS_HEADER = "x-aex-headers";
export const PROXY_RESPONSE_MODE_HEADER = "x-aex-response-mode";

export const PROXY_ALLOWED_METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"] as const;
export type ProxyMethod = (typeof PROXY_ALLOWED_METHODS)[number];

export const PROXY_RESPONSE_MODES = ["status_only", "headers_only", "full"] as const;
export type ProxyResponseMode = (typeof PROXY_RESPONSE_MODES)[number];

export const PROXY_RETRY_JITTERS = ["full", "none"] as const;
export type ProxyRetryJitter = (typeof PROXY_RETRY_JITTERS)[number];

export interface ProxyRetryPolicy {
  /** Total attempts, including the initial request. */
  readonly maxAttempts?: number;
  readonly initialDelayMs?: number;
  readonly maxDelayMs?: number;
  readonly jitter?: ProxyRetryJitter;
  readonly retryOnStatuses?: readonly number[];
  readonly retryOnMethods?: readonly ProxyMethod[];
  readonly respectRetryAfter?: boolean;
}

export type ResolvedProxyRetryPolicy = Required<ProxyRetryPolicy>;

export const PROXY_RETRY_POLICY_DEFAULTS: ResolvedProxyRetryPolicy = {
  maxAttempts: 3,
  initialDelayMs: 250,
  maxDelayMs: 5000,
  jitter: "full",
  retryOnStatuses: [408, 425, 429, 500, 502, 503, 504],
  retryOnMethods: ["GET", "HEAD"],
  respectRetryAfter: true
} as const;

export function resolveProxyRetryPolicy(policy: ProxyRetryPolicy): ResolvedProxyRetryPolicy {
  return {
    maxAttempts: policy.maxAttempts ?? PROXY_RETRY_POLICY_DEFAULTS.maxAttempts,
    initialDelayMs: policy.initialDelayMs ?? PROXY_RETRY_POLICY_DEFAULTS.initialDelayMs,
    maxDelayMs: policy.maxDelayMs ?? PROXY_RETRY_POLICY_DEFAULTS.maxDelayMs,
    jitter: policy.jitter ?? PROXY_RETRY_POLICY_DEFAULTS.jitter,
    retryOnStatuses: policy.retryOnStatuses ?? PROXY_RETRY_POLICY_DEFAULTS.retryOnStatuses,
    retryOnMethods: policy.retryOnMethods ?? PROXY_RETRY_POLICY_DEFAULTS.retryOnMethods,
    respectRetryAfter: policy.respectRetryAfter ?? PROXY_RETRY_POLICY_DEFAULTS.respectRetryAfter
  };
}

/**
 * Narrowing order: a request may only narrow below the policy ceiling.
 * `status_only` is narrowest, `full` is widest. The BFF computes
 * `narrowest(policyMode, headerMode)` and ignores escalation attempts,
 * auditing them as `mode_clamped`.
 */
const RESPONSE_MODE_WIDTH: Record<ProxyResponseMode, number> = {
  status_only: 0,
  headers_only: 1,
  full: 2
};

/**
 * Returns the narrower of the two response modes (lower width wins).
 * Pure function so the CLI and BFF can both call it without import cycles.
 */
export function narrowResponseMode(policy: ProxyResponseMode, requested: ProxyResponseMode): ProxyResponseMode {
  return RESPONSE_MODE_WIDTH[requested] < RESPONSE_MODE_WIDTH[policy] ? requested : policy;
}

/**
 * Error codes returned by the proxy route. Stable strings — the CLI
 * matches against them in scripts. Adding a new code is non-breaking;
 * removing or renaming an existing code requires a protocol bump.
 */
export const PROXY_ERROR_CODES = [
  "unsupported_protocol",
  "unauthorized",
  "endpoint_not_found",
  "policy_denied",
  "rate_limited",
  "ssrf_denied",
  "upstream_timeout",
  "upstream_error",
  "exceeded_cap",
  "bad_request",
  "internal_error"
] as const;
export type ProxyErrorCode = (typeof PROXY_ERROR_CODES)[number];

/**
 * Shape of the JSON written to the per-run manifest mounted inside
 * the container (`/mnt/session/uploads/aex/index.json`).
 *
 * Always present (every run), regardless of whether any proxy endpoints
 * were declared. With zero endpoints, `endpoints` is `[]` and
 * `proxyBaseUrl` is `null` — this keeps `aex --help` working
 * uniformly and makes the always-on surface observable in tests.
 *
 * Auth values NEVER appear in this file. The file is mounted into the
 * container; treat it as world-readable from the agent's perspective.
 */
export interface ProxyIndexFile {
  readonly protocolVersion: typeof PROXY_PROTOCOL_VERSION;
  readonly runId: string;
  readonly proxyBaseUrl: string | null;
  readonly endpoints: readonly ProxyIndexEntry[];
}

export interface ProxyIndexEntry {
  readonly name: string;
  readonly baseUrl: string;
  readonly authShape: ProxyAuthShape;
  readonly allowMethods: readonly ProxyMethod[];
  readonly allowPathPrefixes: readonly string[];
  readonly allowHeaders: readonly string[];
  readonly responseMode: ProxyResponseMode;
  readonly maxRequestBytes: number;
  readonly maxResponseBytes: number;
  readonly timeoutMs: number;
  readonly retry?: ResolvedProxyRetryPolicy;
}

/**
 * Default caps for a proxy endpoint when the submission doesn't specify one.
 * Lives in the protocol module (next to the index-file shape) so
 * {@link buildProxyIndexFile} can fill every optional cap with a concrete
 * value; the submission parser re-exports it.
 *
 * `maxResponseBytes: 0` means UNLIMITED — the v2 path streams the upstream body
 * unbuffered (O(1) memory regardless of size), so there is no cap to apply by
 * default. A customer can opt into a per-response truncation cap by setting a
 * positive value. There is no cumulative per-run call/byte budget: it needed a
 * per-call counter on the hot path and only existed to bound memory, which
 * streaming already does. The platform records named-proxy request bytes,
 * response bytes, attempts, and retries as run-log usage telemetry only; no
 * pricing/charging model is derived here.
 */
export const PROXY_ENDPOINT_DEFAULTS = {
  allowHeaders: [] as readonly string[],
  responseMode: "headers_only" as ProxyResponseMode,
  // 10 MiB. The body is buffered in the hosted API to enforce this cap, while the
  // launch default fits practical multimodal/tool POSTs without every endpoint
  // needing an override.
  maxRequestBytes: 10 * 1024 * 1024,
  // Unlimited (0). The request body is buffered to enforce its cap, so that
  // stays finite; the response is streamed, so it does not need one.
  maxResponseBytes: 0,
  // 5 minutes. Long-running upstream tool/model calls should not fail under a
  // development-oriented 10s ceiling; endpoints can still set a smaller value.
  timeoutMs: 5 * 60 * 1000
} as const;

/**
 * Non-secret endpoint policy the index builder consumes. Structurally a
 * subset of `PlatformProxyEndpoint` (submission.ts) — declared here so the
 * protocol module stays free of an import cycle with the submission parser.
 */
export interface ProxyEndpointPolicy {
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
  readonly retry?: ProxyRetryPolicy;
}

export interface BuildProxyIndexFileInput {
  readonly runId: string;
  /**
   * Hosted API origin that serves `/api/runs/:runId/proxy/:endpointName`.
   * When unset
   * (or empty) the run has no reachable proxy plane and `proxyBaseUrl`
   * resolves to `null`.
   */
  readonly proxyPublicBaseUrl?: string;
  readonly endpoints?: readonly ProxyEndpointPolicy[];
}

/**
 * Build the per-run {@link ProxyIndexFile} mounted into the container at
 * `/mnt/session/uploads/aex/index.json`. Pure: applies
 * {@link PROXY_ENDPOINT_DEFAULTS} so every optional cap is concrete, and
 * carries ONLY the non-secret endpoint policy — auth values never appear.
 *
 * ALWAYS emits a file (the always-on surface). With zero endpoints OR no
 * `proxyPublicBaseUrl`, `proxyBaseUrl` is `null` and `endpoints` is `[]`.
 * Otherwise `proxyBaseUrl` is `<trimmed base>/api/runs/<runId>/proxy`, the
 * prefix the in-container runtime bridge appends `/<endpointName>` to (proxy.ts).
 */
export function buildProxyIndexFile(input: BuildProxyIndexFileInput): ProxyIndexFile {
  const endpoints = input.endpoints ?? [];
  const base = input.proxyPublicBaseUrl?.trim() ?? "";
  const haveBase = base.length > 0;
  const proxyBaseUrl =
    endpoints.length > 0 && haveBase
      ? `${base.replace(/\/+$/, "")}/api/runs/${input.runId}/proxy`
      : null;
  return {
    protocolVersion: PROXY_PROTOCOL_VERSION,
    runId: input.runId,
    proxyBaseUrl,
    endpoints: endpoints.map(
      (e): ProxyIndexEntry => ({
        name: e.name,
        baseUrl: e.baseUrl,
        authShape: e.authShape,
        allowMethods: e.allowMethods,
        allowPathPrefixes: e.allowPathPrefixes,
        allowHeaders: e.allowHeaders ?? PROXY_ENDPOINT_DEFAULTS.allowHeaders,
        responseMode: e.responseMode ?? PROXY_ENDPOINT_DEFAULTS.responseMode,
        maxRequestBytes: e.maxRequestBytes ?? PROXY_ENDPOINT_DEFAULTS.maxRequestBytes,
        maxResponseBytes: e.maxResponseBytes ?? PROXY_ENDPOINT_DEFAULTS.maxResponseBytes,
        timeoutMs: e.timeoutMs ?? PROXY_ENDPOINT_DEFAULTS.timeoutMs,
        ...(e.retry !== undefined ? { retry: resolveProxyRetryPolicy(e.retry) } : {})
      })
    )
  };
}

/**
 * Structural description of how the upstream endpoint expects auth.
 * The actual auth value lives in the run's Vault bundle under
 * `secrets.proxyEndpointAuth[i].value` and is never reflected back
 * into the container or index file.
 *
 * The `none` variant declares an upstream that takes no auth (public
 * APIs like Wikimedia Commons or NASA Images). It still routes through
 * the proxy for unified egress and audit, but
 * carries no `proxyEndpointAuth[]` entry and the BFF injects no
 * header or query value.
 */
export type ProxyAuthShape =
  | { readonly type: "none" }
  | { readonly type: "bearer" }
  | { readonly type: "basic" }
  | { readonly type: "header"; readonly name: string }
  | { readonly type: "query"; readonly name: string };

export type ProxyAuthType = ProxyAuthShape["type"];

/**
 * Header name (lowercase) that an upstream auth shape uses as its
 * carrier. Returns `undefined` for query-based and keyless auth.
 *
 * Used by the submission parser to forbid `allowHeaders` from listing
 * the auth header (avoids leaks via caller-supplied headers), and by
 * the proxy route to strip any caller header that would collide with
 * the auth carrier at request time.
 */
export function authShapeHeaderName(shape: ProxyAuthShape): string | undefined {
  switch (shape.type) {
    case "bearer":
    case "basic":
      return "authorization";
    case "header":
      return shape.name.toLowerCase();
    case "query":
    case "none":
      return undefined;
  }
}

/**
 * Query-string key that an upstream query-based auth shape uses as its
 * carrier. Returns `undefined` for non-query shapes (including "none").
 */
export function authShapeQueryName(shape: ProxyAuthShape): string | undefined {
  return shape.type === "query" ? shape.name : undefined;
}

/**
 * Inbound request headers every Aex proxy plane STRIPS before
 * forwarding a runtime/runner request upstream. Three categories:
 *
 *   - Credential carriers (`authorization`, `x-api-key`, `cookie`,
 *     `proxy-authorization`) — these belong to Aex's own auth gate
 *     (the per-run bearer) or to the caller, never the upstream. The
 *     legitimate upstream credential is injected server-side from the
 *     run's Vault bundle / endpoint auth shape AFTER this strip, so it is
 *     never sourced from an inbound header.
 *   - Hop-by-hop fields (RFC 7230 §6.1: `connection`, `keep-alive`,
 *     `transfer-encoding`, `te`, `trailer`, `upgrade`, `expect`,
 *     `proxy-authenticate`, `proxy-connection`) — must not survive a
 *     proxy hop.
 *   - Routing primitives a compromised runner could spoof to bypass an
 *     upstream's IP allowlist / rate-limit (`host`, `content-length`,
 *     `x-forwarded-*`, `x-real-ip`, `forwarded`).
 *
 * The platform API provider-proxy and the dashboard MCP proxy strip exactly
 * this set (both inject upstream auth separately — the provider key, or the
 * Vault MCP-bundle headers, applied AFTER the strip). The dashboard
 * customer HTTP proxy hard-denies this set MINUS `x-api-key`, because a
 * customer endpoint may legitimately declare `x-api-key` as its auth
 * carrier; it derives from this constant so the hop-by-hop + routing
 * entries never drift. Keeping the membership here is the single source of
 * truth that stops those surfaces diverging.
 */
export const PROXY_STRIPPED_INBOUND_HEADERS: ReadonlySet<string> = new Set([
  // credential carriers
  "authorization",
  "x-api-key",
  "cookie",
  "proxy-authorization",
  // hop-by-hop (RFC 7230 §6.1)
  "connection",
  "keep-alive",
  "transfer-encoding",
  "te",
  "trailer",
  "upgrade",
  "expect",
  "proxy-authenticate",
  "proxy-connection",
  // routing primitives a runner could spoof
  "host",
  "content-length",
  "x-forwarded-for",
  "x-forwarded-host",
  "x-forwarded-proto",
  "x-forwarded-port",
  "x-real-ip",
  "forwarded"
]);

/**
 * JSON body returned on a successful proxy call. The actual HTTP
 * response from the BFF to the CLI is always 200 once the BFF accepts
 * the request; the upstream's status/headers/body are reflected inside
 * this envelope so the CLI can decide what to write to stdout/stderr.
 */
export interface ProxyResponseEnvelope {
  readonly endpointName: string;
  readonly upstreamStatus: number;
  /** Lowercase header names → values. Allowlist-filtered by the BFF. */
  readonly upstreamHeaders: Readonly<Record<string, string>>;
  /**
   * Base64-encoded upstream body. Present only when the effective
   * response mode is `full`. Truncated to `maxResponseBytes`; if the
   * upstream exceeded the cap, `truncated` is `true`.
   */
  readonly upstreamBodyBase64?: string;
  readonly truncated?: boolean;
  /**
   * Echoed back so the CLI can warn the agent when its requested mode
   * was clamped against the policy ceiling.
   */
  readonly effectiveResponseMode: ProxyResponseMode;
  readonly modeClamped: boolean;
}

/**
 * JSON body returned on any error. The CLI emits this verbatim on
 * stderr and exits non-zero. Audit row carries the same `code`.
 */
export interface ProxyErrorBody {
  readonly error: ProxyErrorCode;
  /** Human-readable message. Never includes auth values. */
  readonly message: string;
  /**
   * Optional diagnostic fields. Always safe to surface — auth values
   * and full URLs are stripped at the BFF.
   */
  readonly endpointName?: string;
  readonly upstreamStatus?: number;
  /** Server-supplied protocol version on `unsupported_protocol`. */
  readonly serverProtocolVersion?: string;
}

/**
 * Status code → error code mapping used by the BFF to ensure the audit
 * row's error code and the HTTP response line up. Kept here so callers
 * can do a sanity check in tests.
 */
export const PROXY_ERROR_HTTP_STATUS: Record<ProxyErrorCode, number> = {
  unsupported_protocol: 426,
  unauthorized: 401,
  endpoint_not_found: 404,
  policy_denied: 403,
  rate_limited: 429,
  ssrf_denied: 403,
  upstream_timeout: 504,
  upstream_error: 502,
  exceeded_cap: 502,
  bad_request: 400,
  internal_error: 500
};
