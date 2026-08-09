import { parseCredential, type ParsedCredential } from "./credentials.js";
import { resolveCentralBaseUrl, resolveRegionalBaseUrl } from "./routing.js";
import { AexConfigError } from "../transport/errors.js";
import { FetchTransport, type AexTransport, type WireRequest } from "../transport/transport.js";
import { CONTRACT_DIGEST, ROUTES, type RouteId } from "../generated/routes.js";

export interface AexOptions {
  readonly apiKey: string;
  readonly centralBaseUrl?: string;
  readonly regionalBaseUrl?: string;
  readonly workspaceId?: string;
  readonly transport?: AexTransport;
  readonly fetch?: typeof globalThis.fetch;
  readonly signal?: AbortSignal;
}

export interface SessionCreateRequest {
  readonly provider: string;
  readonly model: string;
  readonly credentialId?: string;
  readonly [key: string]: unknown;
}

export class Aex {
  readonly transport: AexTransport;
  readonly sessions: SessionsClient;
  readonly workspaces: WorkspacesClient;
  readonly #credential: ParsedCredential;
  readonly #signal: AbortSignal | undefined;

  constructor(apiKey: string, options?: Omit<AexOptions, "apiKey">);
  constructor(options: AexOptions);
  constructor(apiKeyOrOptions: string | AexOptions, rest: Omit<AexOptions, "apiKey"> = {}) {
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
    this.sessions = new SessionsClient(this);
    this.workspaces = new WorkspacesClient(this);
  }

  async execute<T>(routeId: RouteId, bindings: Readonly<Record<string, string>> = {}, body?: unknown,
    idempotencyKey?: string): Promise<T> {
    const route = ROUTES[routeId];
    let path = route.path;
    for (const [name, value] of Object.entries(bindings)) {
      path = path.replace(`{${name}}`, encodeURIComponent(value));
    }
    const headers = new Headers({
      authorization: this.#credential.authorizationHeader(),
      accept: "application/json",
      "Aex-Client": "aex-sdk/0.50.0",
    });
    if (idempotencyKey) headers.set("Idempotency-Key", idempotencyKey);
    let bytes: Uint8Array | undefined;
    if (body !== undefined) {
      headers.set("content-type", "application/json");
      bytes = encodeKnownBody(routeId, body);
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

export class SessionsClient {
  readonly #aex: Aex;
  constructor(aex: Aex) { this.#aex = aex; }

  async create<T = unknown>(request: SessionCreateRequest, options: { idempotencyKey?: string } = {}): Promise<T> {
    if (!request.provider || !request.model) {
      throw new AexConfigError("session create requires an explicit provider and model");
    }
    return this.#aex.execute<T>("session_create", {}, request, options.idempotencyKey);
  }

  async get<T = unknown>(sessionId: string): Promise<T> {
    return this.#aex.execute<T>("session_get", { sessionId });
  }
}

export class WorkspacesClient {
  readonly #aex: Aex;
  constructor(aex: Aex) { this.#aex = aex; }

  async get<T = unknown>(workspaceId: string): Promise<T> {
    return this.#aex.execute<T>("workspace_get", { workspaceId });
  }
}

function encodeKnownBody(routeId: RouteId, body: unknown): Uint8Array {
  if (routeId !== "session_create" || typeof body !== "object" || body === null) {
    throw new AexConfigError("this temporary client boundary cannot encode the requested body");
  }
  const request = body as SessionCreateRequest;
  const ordered = {
    ...(request.credentialId === undefined ? {} : { credentialId: request.credentialId }),
    model: request.model,
    provider: request.provider,
  };
  return new TextEncoder().encode(JSON.stringify(ordered));
}
