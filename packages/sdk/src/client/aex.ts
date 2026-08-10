import { parseCredential, type ParsedCredential } from "./credentials.js";
import { resolveCentralBaseUrl, resolveRegionalBaseUrl } from "./routing.js";
import { FetchTransport, type AexTransport, type WireRequest } from "../transport/transport.js";
import { CONTRACT_DIGEST, ROUTES, type RouteId } from "../generated/routes.js";
import { GeneratedResources, type ExecuteOptions } from "../generated/resources.js";

export interface AexOptions {
  readonly apiKey: string;
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
  readonly #credential: ParsedCredential;
  readonly #signal: AbortSignal | undefined;

  constructor(apiKey: string, options?: Omit<AexOptions, "apiKey">);
  constructor(options: AexOptions);
  constructor(apiKeyOrOptions: string | AexOptions, rest: Omit<AexOptions, "apiKey"> = {}) {
    super();
    const options = typeof apiKeyOrOptions === "string"
      ? { ...rest, apiKey: apiKeyOrOptions }
      : apiKeyOrOptions;
    this.#credential = parseCredential(options.apiKey);
    this.#signal = options.signal;
    if (options.transport) {
      this.transport = options.transport;
    } else {
      this.transport = new FetchTransport({
        central: resolveCentralBaseUrl(options.centralBaseUrl),
        regional: resolveRegionalBaseUrl(this.#credential, options),
      }, options.fetch);
    }
  }

  async execute<T>(routeId: RouteId, bindings: Readonly<Record<string, string>> = {},
    options: ExecuteOptions = {}): Promise<T> {
    const route = ROUTES[routeId];
    let path = route.path;
    for (const [name, value] of Object.entries(bindings)) {
      path = path.replace(`{${name}}`, encodeURIComponent(value));
    }
    const query = new URLSearchParams(options.query ?? {}).toString();
    if (query) path = `${path}?${query}`;
    const headers = new Headers({
      authorization: this.#credential.authorizationHeader(),
      accept: "application/json",
      "Aex-Client": "aex-sdk/0.50.0",
    });
    if (options.idempotencyKey) headers.set("Idempotency-Key", options.idempotencyKey);
    if (options.operationId) headers.set("Aex-Operation-Id", options.operationId);
    let bytes: Uint8Array | undefined;
    if (options.body !== undefined) {
      headers.set("content-type", "application/json");
      bytes = encodeCanonicalJson(options.body);
    }
    const request: WireRequest = {
      routeId,
      method: route.method,
      path,
      headers,
      ...(bytes ? { body: bytes } : {}),
      ...(this.#signal ? { signal: this.#signal } : {}),
    };
    const response = await this.transport.execute<T>(request);
    return response.body;
  }

  static buildInfo(): Readonly<{ packageVersion: string; contractDigest: string; sourceSha: string }> {
    return Object.freeze({ packageVersion: "0.50.0", contractDigest: CONTRACT_DIGEST, sourceSha: "development" });
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
