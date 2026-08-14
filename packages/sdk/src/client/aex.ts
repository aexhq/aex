import { DashboardSession, WorkspaceApiKey } from "./credentials.js";
import { resolveCentralBaseUrl, resolveRegionalBaseUrl } from "./routing.js";
import { FetchTransport, type AexTransport, type WireRequest } from "../transport/transport.js";
import { CONTRACT_DIGEST, ROUTES, type RouteId } from "../generated/routes.js";
import { GeneratedResources, type ExecuteOptions } from "../generated/resources.js";
import { WorkspaceFiles } from "../files/workspace-files.js";
import { apiErrorFromResponse } from "../transport/errors.js";

const SDK_VERSION = "0.53.1";

export interface AexOptions {
  readonly apiKey?: string;
  readonly dashboardSession?: string;
  readonly centralBaseUrl?: string;
  readonly regionalBaseUrl?: string;
  readonly workspaceId?: string;
  readonly transport?: AexTransport;
  readonly fetch?: typeof globalThis.fetch;
  readonly signal?: AbortSignal;
}

/**
 * The client.
 *
 * The resource namespaces come from `GeneratedResources`, which the contract
 * generator writes: one method per served operation, and none for an operation
 * the platform does not answer. `execute` stays total over `RouteId`, so a
 * deferred or streaming operation is still callable by anyone who wants to see
 * what the platform actually says.
 */
export class Aex extends GeneratedResources {
  readonly transport: AexTransport;
  /** Latest-only workspace files, including verified direct upload/download helpers. */
  readonly workspaceFiles: WorkspaceFiles;
  readonly #apiKey: WorkspaceApiKey | undefined;
  readonly #dashboardSession: DashboardSession | undefined;
  readonly #signal: AbortSignal | undefined;

  constructor(apiKey: string, options?: Omit<AexOptions, "apiKey">);
  constructor(options: AexOptions);
  constructor(apiKeyOrOptions: string | AexOptions, rest: Omit<AexOptions, "apiKey"> = {}) {
    super();
    const options = typeof apiKeyOrOptions === "string"
      ? { ...rest, apiKey: apiKeyOrOptions }
      : apiKeyOrOptions;
    this.#apiKey = options.apiKey ? WorkspaceApiKey.parse(options.apiKey) : undefined;
    this.#dashboardSession = options.dashboardSession
      ? DashboardSession.parse(options.dashboardSession)
      : undefined;
    if (!this.#apiKey && !this.#dashboardSession) {
      throw new TypeError("Aex requires apiKey or dashboardSession");
    }
    this.#signal = options.signal;
    if (options.transport) {
      this.transport = options.transport;
    } else {
      const regional = resolveRegionalBaseUrl(this.#apiKey ?? this.#dashboardSession!, options);
      this.transport = new FetchTransport({
        central: resolveCentralBaseUrl(options.centralBaseUrl),
        ...(regional ? { regional } : {}),
      }, options.fetch);
    }
    this.workspaceFiles = new WorkspaceFiles(this, options.fetch);
  }

  async execute<T>(routeId: RouteId, bindings: Readonly<Record<string, string>> = {},
    options: ExecuteOptions = {}): Promise<T> {
    const request = this.#request(routeId, bindings, options);
    const response = await this.transport.execute<T>(request);
    if (response.status < 200 || response.status >= 300) {
      throw apiErrorFromResponse(routeId, response.status, response.body, response.headers);
    }
    return response.body;
  }

  async *stream<T>(routeId: RouteId, bindings: Readonly<Record<string, string>> = {},
    options: ExecuteOptions = {}): AsyncIterable<T> {
    const request = this.#request(routeId, bindings, options);
    const response = await this.transport.stream<T>(request);
    if (response.status < 200 || response.status >= 300) {
      throw apiErrorFromResponse(routeId, response.status, response.error, response.headers);
    }
    if (!response.frames) throw new TypeError(`stream route ${routeId} returned no frame body`);
    yield* response.frames;
  }

  #request(
    routeId: RouteId,
    bindings: Readonly<Record<string, string>>,
    options: ExecuteOptions,
  ): WireRequest {
    const route = ROUTES[routeId];
    let path = route.path;
    for (const [name, value] of Object.entries(bindings)) {
      path = path.replace(`{${name}}`, encodeURIComponent(value));
    }
    const query = new URLSearchParams(options.query ?? {}).toString();
    if (query) path = `${path}?${query}`;
    const transport = route.transport as string;
    const credential = route.plane === "regional" ? this.#apiKey : this.#dashboardSession;
    if (!credential && routeId !== "dashboard_session_create") {
      throw new TypeError(
        route.plane === "regional"
          ? "a workspace API key is required for regional routes"
          : "a dashboard session is required for central routes",
      );
    }
    const headers = new Headers({
      accept: transport === "binary"
        ? "application/octet-stream"
        : transport === "ndjson"
          ? "application/x-ndjson"
          : "application/json",
      "Aex-Client": `aex-sdk/${SDK_VERSION}`,
    });
    if (credential) headers.set("authorization", credential.authorizationHeader());
    if (options.idempotencyKey) headers.set("Idempotency-Key", options.idempotencyKey);
    if (options.operationId) headers.set("Aex-Operation-Id", options.operationId);
    let bytes: Uint8Array | undefined;
    if (options.body !== undefined) {
      if (route.bodyClass as string === "binary") {
        if (!(options.body instanceof Uint8Array)) {
          throw new TypeError(`binary route ${routeId} requires a Uint8Array body`);
        }
        headers.set("content-type", "application/octet-stream");
        bytes = options.body;
      } else {
        headers.set("content-type", "application/json");
        bytes = encodeCanonicalJson(options.body);
      }
    }
    return {
      routeId,
      method: route.method,
      path,
      headers,
      ...(bytes ? { body: bytes } : {}),
      ...(this.#signal ? { signal: this.#signal } : {}),
    };
  }

  static buildInfo(): Readonly<{ packageVersion: string; contractDigest: string; sourceSha: string }> {
    return Object.freeze({ packageVersion: SDK_VERSION, contractDigest: CONTRACT_DIGEST, sourceSha: "development" });
  }
}

/**
 * Encodes one request body with object members in sorted order.
 *
 * The platform derives a replay intent from the bytes it receives, so two calls
 * a caller believes are identical must serialize identically. Ordering every
 * object once here is the rule that makes that true; the alternative is a
 * per-route encoder, which is what this replaced.
 */
function encodeCanonicalJson(body: unknown): Uint8Array {
  return new TextEncoder().encode(JSON.stringify(canonical(body)));
}

function canonical(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonical);
  if (value === null || typeof value !== "object") return value;
  const ordered: Record<string, unknown> = {};
  for (const key of Object.keys(value as Record<string, unknown>).sort()) {
    const member = (value as Record<string, unknown>)[key];
    if (member !== undefined) ordered[key] = canonical(member);
  }
  return ordered;
}
