import { ROUTES, type RouteId } from "../generated/routes.js";
import { AexConfigError, AexStreamProtocolError } from "./errors.js";

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

export interface WireStreamResponse<T = unknown> {
  readonly status: number;
  readonly headers: Headers;
  readonly frames?: AsyncIterable<T>;
  readonly error?: unknown;
}

export interface AexTransport {
  execute<T>(request: WireRequest): Promise<WireResponse<T>>;
  stream<T>(request: WireRequest): Promise<WireStreamResponse<T>>;
}

export class FetchTransport implements AexTransport {
  readonly #baseUrls: Readonly<{ central: string; regional?: string }>;
  readonly #fetch: typeof globalThis.fetch;

  constructor(baseUrls: { central: string; regional?: string }, fetchLike = globalThis.fetch) {
    this.#baseUrls = baseUrls;
    this.#fetch = fetchLike.bind(globalThis);
  }

  async execute<T>(request: WireRequest): Promise<WireResponse<T>> {
    const plane = ROUTES[request.routeId].plane;
    const baseUrl = this.#baseUrls[plane];
    if (!baseUrl) throw new AexConfigError("a workspace API key is required for regional routes");
    const response = await this.#fetch(`${baseUrl}${request.path}`, {
      method: request.method,
      headers: request.headers,
      ...(request.body ? { body: toFetchBody(request.body) } : {}),
      ...(request.signal ? { signal: request.signal } : {}),
      redirect: "error",
    });
    const binary = ROUTES[request.routeId].transport as string === "binary";
    const body = response.status === 204
      ? undefined
      : response.ok && binary
        ? new Uint8Array(await response.arrayBuffer())
        : await response.json();
    return { status: response.status, headers: response.headers, body: body as T };
  }

  async stream<T>(request: WireRequest): Promise<WireStreamResponse<T>> {
    const plane = ROUTES[request.routeId].plane;
    const baseUrl = this.#baseUrls[plane];
    if (!baseUrl) throw new AexConfigError("a workspace API key is required for regional routes");
    const response = await this.#fetch(`${baseUrl}${request.path}`, {
      method: request.method,
      headers: request.headers,
      ...(request.body ? { body: toFetchBody(request.body) } : {}),
      ...(request.signal ? { signal: request.signal } : {}),
      redirect: "error",
    });
    if (!response.ok) {
      return {
        status: response.status,
        headers: response.headers,
        error: await response.json(),
      };
    }
    if (!response.body) {
      return {
        status: response.status,
        headers: response.headers,
        frames: emptyFrames<T>(),
      };
    }
    const contentType = response.headers.get("content-type")?.split(";", 1)[0]?.trim();
    if (contentType !== "application/x-ndjson") {
      throw new AexStreamProtocolError("stream response was not application/x-ndjson");
    }
    return {
      status: response.status,
      headers: response.headers,
      frames: parseResponseNdjson<T>(response.body),
    };
  }
}

async function* emptyFrames<T>(): AsyncIterable<T> {}

async function* parseResponseNdjson<T>(body: ReadableStream<Uint8Array>): AsyncIterable<T> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let pending = "";
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      pending += decoder.decode(value, { stream: true });
      for (;;) {
        const newline = pending.indexOf("\n");
        if (newline < 0) break;
        const line = pending.slice(0, newline).trim();
        pending = pending.slice(newline + 1);
        if (line) yield parseFrame<T>(line);
      }
    }
    pending += decoder.decode();
    const final = pending.trim();
    if (final) yield parseFrame<T>(final);
  } finally {
    reader.releaseLock();
  }
}

function parseFrame<T>(line: string): T {
  try {
    return JSON.parse(line) as T;
  } catch {
    throw new AexStreamProtocolError("stream contained an invalid NDJSON frame");
  }
}

function toFetchBody(bytes: Uint8Array): Uint8Array<ArrayBuffer> {
  const buffer = bytes.buffer;
  return buffer instanceof ArrayBuffer
    ? new Uint8Array(buffer, bytes.byteOffset, bytes.byteLength)
    : Uint8Array.from(bytes);
}
