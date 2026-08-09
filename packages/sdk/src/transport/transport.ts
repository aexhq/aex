import { ROUTES, type RouteId } from "../generated/routes.js";

export interface WireRequest {
  readonly routeId: RouteId;
  readonly method: string;
  readonly path: string;
  readonly headers: Headers;
  readonly body?: Uint8Array;
  readonly signal?: AbortSignal;
}

export interface WireResponse<T = unknown> {
  readonly status: number;
  readonly headers: Headers;
  readonly body: T;
}

export interface AexTransport {
  execute<T>(request: WireRequest): Promise<WireResponse<T>>;
}

export class FetchTransport implements AexTransport {
  readonly #baseUrls: Readonly<{ central: string; regional: string }>;
  readonly #fetch: typeof globalThis.fetch;

  constructor(baseUrls: { central: string; regional: string }, fetchLike = globalThis.fetch) {
    this.#baseUrls = baseUrls;
    this.#fetch = fetchLike.bind(globalThis);
  }

  async execute<T>(request: WireRequest): Promise<WireResponse<T>> {
    const plane = ROUTES[request.routeId].plane;
    const response = await this.#fetch(`${this.#baseUrls[plane]}${request.path}`, {
      method: request.method,
      headers: request.headers,
      ...(request.body ? { body: request.body } : {}),
      ...(request.signal ? { signal: request.signal } : {}),
      redirect: "error",
    });
    const body = response.status === 204 ? undefined : await response.json();
    return { status: response.status, headers: response.headers, body: body as T };
  }
}
