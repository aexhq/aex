import { AexApiError, AexError, AexNetworkError, redactUrl } from "./sdk-errors.js";
import { AEX_DEFAULT_BASE_URL } from "./stable.js";

export type FetchLike = (input: string | URL | Request, init?: RequestInit) => Promise<Response>;

/**
 * Sink for local debug traces. Receives one preformatted line per HTTP
 * round-trip (method, path, status, elapsed). NEVER carries the auth
 * header, request/response bodies, or query string — purely a local
 * diagnostic; nothing is uploaded. The SDK wires this to `console.error`
 * when `debug` is set; the CLI wires it to stderr under `--debug`.
 */
export type DebugSink = (line: string) => void;

export interface HttpClientOptions {
  /**
   * API plane root. Optional — defaults to `AEX_DEFAULT_BASE_URL`
   * (`https://api.aex.dev`). Self-hosted deployments override with their
   * own URL; no env var consults this value.
   */
  readonly baseUrl?: string;
  readonly apiToken: string;
  readonly fetch?: FetchLike;
  /** When set, every request emits a redacted one-line trace here. */
  readonly debug?: DebugSink;
}

/**
 * Thin transport used by every BFF-bound operation. The SDK class and
 * the CLI subcommands BOTH build an `HttpClient` and pass it to the
 * operations module — so they cannot drift in how they auth, encode
 * query parameters, or decode error responses.
 */
export class HttpClient {
  readonly #baseUrl: URL;
  readonly #apiToken: string;
  readonly #fetch: FetchLike;
  readonly #debug: DebugSink | undefined;

  constructor(options: HttpClientOptions) {
    if (!options.apiToken) {
      throw new Error("HttpClient: apiToken is required");
    }
    const raw = options.baseUrl ?? AEX_DEFAULT_BASE_URL;
    const normalized = raw.endsWith("/") ? raw : `${raw}/`;
    try {
      this.#baseUrl = new URL(normalized);
    } catch (err) {
      throw new Error(
        `HttpClient: invalid aex baseUrl ${JSON.stringify(redactUrl(raw))} — ` +
          `expected an absolute URL like "${AEX_DEFAULT_BASE_URL}"`,
        { cause: err }
      );
    }
    this.#apiToken = options.apiToken;
    this.#fetch = options.fetch ?? fetch;
    this.#debug = options.debug;
  }

  /** Emit a redacted round-trip trace (no auth header, body, or query). */
  #trace(method: string | undefined, url: URL, status: number, startedMs: number): void {
    this.#debug?.(`[aex] ${(method ?? "GET").toUpperCase()} ${url.pathname} -> ${status} ${Date.now() - startedMs}ms`);
  }

  async request<T>(
    path: string,
    init: RequestInit = {},
    query: Record<string, string> = {}
  ): Promise<T> {
    const url = new URL(path.replace(/^\//, ""), this.#baseUrl);
    for (const [key, value] of Object.entries(query)) {
      url.searchParams.set(key, value);
    }
    const headers: Record<string, string> = {
      accept: "application/json",
      authorization: `Bearer ${this.#apiToken}`,
      ...normalizeHeaders(init.headers)
    };
    if (init.body !== undefined && init.body !== null && !headers["content-type"]) {
      // Default to JSON only for string-shaped bodies. FormData / Blob /
      // ArrayBuffer / streams set their own content-type (and FormData
      // specifically needs fetch to compute the multipart boundary), so
      // we leave content-type untouched for non-string bodies.
      if (typeof init.body === "string") {
        headers["content-type"] = "application/json";
      }
    }
    const startedMs = Date.now();
    let response: Response;
    try {
      response = await this.#fetch(url, { ...init, headers });
    } catch (err) {
      throw toNetworkError(init.method, url, err);
    }
    this.#trace(init.method, url, response.status, startedMs);
    const body = await readJson(response);
    if (!response.ok) {
      throw new AexApiError(response.status, extractErrorMessage(body), body);
    }
    return body as T;
  }

  async download(
    path: string,
    init: RequestInit = {},
    query: Record<string, string> = {}
  ): Promise<{ readonly response: Response }> {
    const url = new URL(path.replace(/^\//, ""), this.#baseUrl);
    for (const [key, value] of Object.entries(query)) {
      url.searchParams.set(key, value);
    }
    const headers: Record<string, string> = {
      authorization: `Bearer ${this.#apiToken}`,
      ...normalizeHeaders(init.headers)
    };
    const startedMs = Date.now();
    let response: Response;
    try {
      response = await this.#fetch(url, { ...init, headers });
    } catch (err) {
      throw toNetworkError(init.method, url, err);
    }
    this.#trace(init.method, url, response.status, startedMs);
    if (!response.ok) {
      const body = await readJson(response);
      throw new AexApiError(response.status, extractErrorMessage(body), body);
    }
    return { response };
  }
}

/**
 * Wrap a fetch rejection into an {@link AexNetworkError} carrying the
 * request's method + redacted host/path. Caller-initiated aborts and
 * already-structured aex errors (e.g. a retry layer's AexRateLimitError or
 * AexNetworkError) pass through untouched.
 */
function toNetworkError(method: string | undefined, url: URL, err: unknown): unknown {
  if (err instanceof AexError) return err;
  // `DOMException` is not an `Error` subclass on every runtime, so match aborts by name.
  if ((err as { readonly name?: unknown } | null | undefined)?.name === "AbortError") return err;
  return new AexNetworkError({
    method: (method ?? "GET").toUpperCase(),
    host: url.host,
    path: url.pathname,
    cause: err
  });
}

function normalizeHeaders(headers: HeadersInit | undefined): Record<string, string> {
  if (!headers) return {};
  if (headers instanceof Headers) return Object.fromEntries(headers.entries());
  if (Array.isArray(headers)) return Object.fromEntries(headers);
  return headers;
}

async function readJson(response: Response): Promise<unknown> {
  const text = await response.text();
  if (text.length === 0) return {};
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return { raw: text };
  }
}

function extractErrorMessage(body: unknown): string {
  if (body && typeof body === "object") {
    const obj = body as { readonly error?: unknown; readonly message?: unknown };
    if (typeof obj.error === "string") {
      // A 409 `session_busy` body carries the session's CURRENT status.
      // Surface it: a send to a deleted (or cancelling/suspending) session
      // otherwise reads as merely "busy", which is misleading for a session
      // that will never accept a turn again.
      const status = (body as { readonly status?: unknown }).status;
      if (obj.error === "session_busy" && typeof status === "string") {
        return `session_busy (session status: ${status})`;
      }
      return obj.error;
    }
    if (obj.error && typeof obj.error === "object" && "message" in obj.error) {
      const message = (obj.error as { readonly message?: unknown }).message;
      if (typeof message === "string") return message;
    }
    // aex API error envelope: `{ ok:false, code, message }`. Surface
    // the server's message so structured rejections (e.g. runtime support)
    // aren't flattened to the generic fallback below.
    if (typeof obj.message === "string") return obj.message;
  }
  return "aex API request failed";
}
