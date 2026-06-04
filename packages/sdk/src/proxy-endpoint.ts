import {
  authShapeHeaderName,
  PROXY_RESPONSE_MODES,
  type PlatformProxyAuthValue,
  type PlatformProxyEndpoint,
  type PlatformProxyEndpointAuth,
  type ProxyAuthShape,
  type ProxyMethod,
  type ProxyResponseMode
} from "@aexhq/contracts";

/**
 * Constructor-only surface for declaring a per-run HTTP proxy endpoint.
 *
 * Why a class rather than a raw object literal: the raw
 * `PlatformProxyEndpoint` + `PlatformProxyEndpointAuth` shape lets the
 * caller assemble two halves of one logical thing (the auth shape and
 * the auth value), and they often drift — agents writing freeform
 * objects regularly hit runtime rejections with `responseMode: "json"`
 * (real values are `status_only | headers_only | full`) or
 * `authShape: { type: "header", header: "X-Api-Key" }` (real field is
 * `name`). The constructor signatures here put the auth secret on the
 * same call as the shape, so any drift becomes a TypeScript error at
 * the call site, not an HTTP 400 a round-trip later.
 *
 * Wire-format unchanged: the SDK splits each `ProxyEndpoint` instance
 * into a `PlatformProxyEndpoint` (the non-secret declaration) plus a
 * `PlatformProxyEndpointAuth` entry (the per-request secret) at
 * `submitRun` time, exactly the way `McpServer` already splits
 * `headers` into `secrets.mcpServers`.
 *
 * Five named constructors:
 *
 *   - `ProxyEndpoint.none  ({ name, baseUrl, ... })` — keyless upstream
 *   - `ProxyEndpoint.bearer({ name, baseUrl, token, ... })`
 *   - `ProxyEndpoint.header({ name, baseUrl, header, value, ... })`
 *   - `ProxyEndpoint.basic ({ name, baseUrl, username, password, ... })`
 *   - `ProxyEndpoint.query ({ name, baseUrl, query, value, ... })`
 *
 * All five share the same allow-list / response-mode / cap parameters.
 * The four authenticated variants split into `secrets.proxyEndpointAuth[].value`
 * at submit time; `none` produces only a declaration (no secret).
 */

export interface ProxyEndpointCommonOptions {
  /**
   * Endpoint name. Lowercase letters, digits, `_`, `-`, up to 63
   * chars. Matches the BFF's `PROXY_ENDPOINT_NAME_PATTERN`. Used as
   * the path segment in `/api/runs/:runId/proxy/:name` and as the
   * cross-reference key in `secrets.proxyEndpointAuth`.
   */
  readonly name: string;
  /** Upstream base URL. Must be an absolute `https://` URL. */
  readonly baseUrl: string;
  /** HTTP methods the in-container caller is allowed to use. */
  readonly allowMethods: readonly ProxyMethod[];
  /** Path prefixes the in-container caller is allowed to hit. */
  readonly allowPathPrefixes: readonly string[];
  /**
   * Caller-supplied headers permitted to pass through the proxy. The
   * auth header (Bearer/Basic/custom-name) is enforced by the proxy
   * itself and MUST NOT appear here.
   */
  readonly allowHeaders?: readonly string[];
  /** Default narrowest response disclosure mode. */
  readonly responseMode?: ProxyResponseMode;
  readonly maxRequestBytes?: number;
  readonly maxResponseBytes?: number;
  readonly timeoutMs?: number;
  readonly perCallBudget?: number;
  readonly responseByteBudget?: number;
}

export interface BearerProxyEndpointOptions extends ProxyEndpointCommonOptions {
  /** Bearer token sent as `Authorization: Bearer <token>`. */
  readonly token: string;
}

/**
 * Options for `ProxyEndpoint.none` — keyless upstream. Same shape as
 * `ProxyEndpointCommonOptions`; declared as an empty extension so the
 * call site reads symmetrically with the other constructors and stays
 * a stable extension point if a future field becomes auth-shape-only.
 */
// deno-lint-ignore no-empty-interface
export interface NoneProxyEndpointOptions extends ProxyEndpointCommonOptions {}

export interface BasicProxyEndpointOptions extends ProxyEndpointCommonOptions {
  readonly username: string;
  readonly password: string;
}

export interface HeaderProxyEndpointOptions extends ProxyEndpointCommonOptions {
  /** Header name the upstream expects the auth value in. */
  readonly header: string;
  /** Header value (the actual secret). */
  readonly value: string;
}

export interface QueryProxyEndpointOptions extends ProxyEndpointCommonOptions {
  /** Query parameter name the upstream expects the auth value in. */
  readonly query: string;
  /** Query parameter value (the actual secret). */
  readonly value: string;
}

export class ProxyEndpoint {
  /** Non-secret half of the wire shape — declaration only. */
  readonly declaration: PlatformProxyEndpoint;
  /**
   * Per-request secret half — auth value keyed by the same `name`.
   * `null` for keyless (`ProxyEndpoint.none`) endpoints; `splitProxyEndpoints`
   * filters those out so they never land in `secrets.proxyEndpointAuth`.
   */
  readonly auth: PlatformProxyEndpointAuth | null;

  private constructor(declaration: PlatformProxyEndpoint, auth: PlatformProxyEndpointAuth | null) {
    this.declaration = declaration;
    this.auth = auth;
  }

  /**
   * Keyless endpoint. Routes through the aex managed proxy for
   * unified egress, audit, and budget enforcement, but the BFF injects
   * no auth header or query parameter. Use for public APIs (Wikimedia
   * Commons, NASA Images, Library of Congress, NARA, GDELT, etc.).
   */
  static none(options: NoneProxyEndpointOptions): ProxyEndpoint {
    const common = buildCommonDeclaration(options, { type: "none" });
    return new ProxyEndpoint(common, null);
  }

  /** Bearer-token endpoint. */
  static bearer(options: BearerProxyEndpointOptions): ProxyEndpoint {
    const common = buildCommonDeclaration(options, { type: "bearer" });
    requireNonEmpty(options.token, "ProxyEndpoint.bearer: token is required");
    return new ProxyEndpoint(common, {
      name: common.name,
      value: { type: "bearer", token: options.token }
    });
  }

  /** Basic-auth endpoint. */
  static basic(options: BasicProxyEndpointOptions): ProxyEndpoint {
    const common = buildCommonDeclaration(options, { type: "basic" });
    requireNonEmpty(options.username, "ProxyEndpoint.basic: username is required");
    requireNonEmpty(options.password, "ProxyEndpoint.basic: password is required");
    return new ProxyEndpoint(common, {
      name: common.name,
      value: { type: "basic", username: options.username, password: options.password }
    });
  }

  /** Custom-header endpoint. */
  static header(options: HeaderProxyEndpointOptions): ProxyEndpoint {
    requireNonEmpty(options.header, "ProxyEndpoint.header: header is required");
    const common = buildCommonDeclaration(options, { type: "header", name: options.header });
    requireNonEmpty(options.value, "ProxyEndpoint.header: value is required");
    return new ProxyEndpoint(common, {
      name: common.name,
      value: { type: "header", value: options.value }
    });
  }

  /** Query-string-auth endpoint. */
  static query(options: QueryProxyEndpointOptions): ProxyEndpoint {
    requireNonEmpty(options.query, "ProxyEndpoint.query: query is required");
    const common = buildCommonDeclaration(options, { type: "query", name: options.query });
    requireNonEmpty(options.value, "ProxyEndpoint.query: value is required");
    return new ProxyEndpoint(common, {
      name: common.name,
      value: { type: "query", value: options.value }
    });
  }
}

/**
 * Split a list of `ProxyEndpoint` instances into the public declarations
 * (`proxyEndpoints[]`) and the per-request auth bundle
 * (`secrets.proxyEndpointAuth[]`). Mirrors the way `submitRun` already
 * splits `McpServer.headers` into `secrets.mcpServers[]`.
 *
 * Throws on duplicate endpoint names — names are the cross-reference
 * key between the two halves and the BFF rejects collisions; failing
 * here gives the caller a precise message at the call site instead of
 * an opaque HTTP error.
 */
export function splitProxyEndpoints(inputs: readonly ProxyEndpoint[]): {
  endpoints: readonly PlatformProxyEndpoint[];
  auth: readonly PlatformProxyEndpointAuth[];
} {
  if (inputs.length === 0) {
    return { endpoints: [], auth: [] };
  }
  const endpoints: PlatformProxyEndpoint[] = [];
  const auth: PlatformProxyEndpointAuth[] = [];
  const seen = new Set<string>();
  for (let i = 0; i < inputs.length; i++) {
    const entry = inputs[i];
    if (!(entry instanceof ProxyEndpoint)) {
      throw new TypeError(
        `proxyEndpoints[${i}] must be a ProxyEndpoint built via ProxyEndpoint.none / bearer / header / basic / query`
      );
    }
    if (seen.has(entry.declaration.name)) {
      throw new Error(`proxyEndpoints duplicate name: ${entry.declaration.name}`);
    }
    seen.add(entry.declaration.name);
    endpoints.push(entry.declaration);
    if (entry.auth !== null) {
      auth.push(entry.auth);
    }
  }
  return { endpoints, auth };
}

function buildCommonDeclaration(
  options: ProxyEndpointCommonOptions,
  authShape: ProxyAuthShape
): PlatformProxyEndpoint {
  requireNonEmpty(options.name, "ProxyEndpoint: name is required");
  requireNonEmpty(options.baseUrl, "ProxyEndpoint: baseUrl is required");
  if (!Array.isArray(options.allowMethods) || options.allowMethods.length === 0) {
    throw new Error("ProxyEndpoint: allowMethods must be a non-empty array");
  }
  if (!Array.isArray(options.allowPathPrefixes) || options.allowPathPrefixes.length === 0) {
    throw new Error("ProxyEndpoint: allowPathPrefixes must be a non-empty array");
  }
  if (options.responseMode !== undefined && !PROXY_RESPONSE_MODES.includes(options.responseMode)) {
    throw new Error(
      `ProxyEndpoint: responseMode must be one of ${PROXY_RESPONSE_MODES.join(" | ")}`
    );
  }
  // Defence in depth: don't let the caller list the auth carrier
  // header in `allowHeaders`. The BFF rejects this too, but a precise
  // SDK-side error message is clearer.
  if (options.allowHeaders) {
    const carrier = authShapeHeaderName(authShape);
    if (carrier) {
      for (const h of options.allowHeaders) {
        if (h.toLowerCase() === carrier) {
          throw new Error(
            `ProxyEndpoint: allowHeaders MUST NOT include the auth header "${h}" — ` +
              `it's set by the proxy on every request`
          );
        }
      }
    }
  }
  return {
    name: options.name,
    baseUrl: options.baseUrl,
    authShape,
    allowMethods: options.allowMethods,
    allowPathPrefixes: options.allowPathPrefixes,
    ...(options.allowHeaders ? { allowHeaders: options.allowHeaders } : {}),
    ...(options.responseMode ? { responseMode: options.responseMode } : {}),
    ...(options.maxRequestBytes !== undefined ? { maxRequestBytes: options.maxRequestBytes } : {}),
    ...(options.maxResponseBytes !== undefined ? { maxResponseBytes: options.maxResponseBytes } : {}),
    ...(options.timeoutMs !== undefined ? { timeoutMs: options.timeoutMs } : {}),
    ...(options.perCallBudget !== undefined ? { perCallBudget: options.perCallBudget } : {}),
    ...(options.responseByteBudget !== undefined
      ? { responseByteBudget: options.responseByteBudget }
      : {})
  };
}

function requireNonEmpty(value: unknown, message: string): asserts value is string {
  if (typeof value !== "string" || !value) {
    throw new Error(message);
  }
}

// Re-export the wire type with a name that's hard to confuse with the
// new SDK class — callers needing the raw discriminated union can
// import it directly from `@aexhq/contracts` as `PlatformProxyAuthValue`.
export type { PlatformProxyAuthValue as ProxyAuthValue };
