import type { ApiError, Event } from "@aexhq/brain/session";
import type { JsonRequestOptions, TransferTicket } from "@aexhq/brain";
import {
  MAX_CREATE_SESSION_REQUEST_BYTES,
  MAX_CUSTOMER_OBSERVATION_BYTES,
  MAX_MESSAGE_REQUEST_BYTES,
  MAX_PUBLIC_EVENT_BYTES,
} from "@aexhq/brain";

import { AbortError, AexError, SessionError, abortError, errorFromApi } from "./errors.js";

export type Fetch = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

export interface EventOptions {
  after?: number;
  follow?: boolean;
  signal?: AbortSignal | undefined;
}

interface ErrorEnvelope {
  error?: ApiError;
}

const MAX_ORDINARY_JSON_BYTES = 2 * 1024 * 1024;
const MAX_ERROR_RESPONSE_BYTES = 64 * 1024;
const CUSTOMER_HAND_OBSERVATION_TIMEOUT_MS = 15_000;

export class Transport {
  readonly baseUrl: string;
  readonly #apiKey: string;
  readonly #fetch: Fetch;

  constructor(apiKey: string, baseUrl: string, fetchImplementation: Fetch) {
    this.#apiKey = apiKey;
    let end = baseUrl.length;
    while (end > 0 && baseUrl.charCodeAt(end - 1) === 47) end -= 1;
    this.baseUrl = baseUrl.slice(0, end);
    this.#fetch = fetchImplementation;
  }

  async customerHandGrant(clientId: string, signal?: AbortSignal): Promise<{
    url: string;
    protocol: string;
    expiresAt: string;
    observationUrl: string;
    observationToken: string;
  }> {
    const grant = await this.json<{
      url: string;
      protocol: string;
      expires_at: string;
      grant_id: string;
      observation_url: string;
      observation_token: string;
    }>(
      "POST",
      "/v1/customer-hand/grants",
      { body: { client_id: clientId }, signal },
    );
    const observationUrl = validateCustomerHandGrant(this.baseUrl, grant);
    return {
      url: grant.url,
      protocol: grant.protocol,
      expiresAt: grant.expires_at,
      observationUrl,
      observationToken: grant.observation_token,
    };
  }

  async customerHandObserve(
    url: string,
    token: string,
    observation: unknown,
  ): Promise<void> {
    const body = encodeJsonOnce(
      observation,
      MAX_CUSTOMER_OBSERVATION_BYTES,
      "Customer Hand observation",
    );
    const response = await this.#fetch(url, {
      method: "POST",
      redirect: "error",
      headers: {
        Accept: "application/json",
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
      },
      body,
      signal: AbortSignal.timeout(CUSTOMER_HAND_OBSERVATION_TIMEOUT_MS),
    });
    if (response.ok) return;
    const preview = await readResponseText(response, 4096)
      .catch(() => "<response too large or unreadable>");
    throw new SessionError(
      `Customer Hand observation ingress returned HTTP ${response.status}: ${preview}`,
      { status: response.status, requestId: response.headers.get("x-request-id") ?? undefined },
    );
  }

  async downloadTransfer(
    ticket: TransferTicket,
    signal?: AbortSignal,
    expectedBytes?: number,
  ): Promise<Uint8Array> {
    const stream = await this.downloadTransferStream(ticket, signal, expectedBytes);
    const reader = stream.getReader();
    const chunks: Uint8Array[] = [];
    let bytes = 0;
    try {
      while (true) {
        const item = await reader.read();
        if (item.done) break;
        bytes += item.value.byteLength;
        chunks.push(item.value);
      }
    } finally {
      reader.releaseLock();
    }
    const content = new Uint8Array(bytes);
    let offset = 0;
    for (const chunk of chunks) {
      content.set(chunk, offset);
      offset += chunk.byteLength;
    }
    return content;
  }

  async downloadTransferStream(
    ticket: TransferTicket,
    signal?: AbortSignal,
    expectedBytes?: number,
  ): Promise<ReadableStream<Uint8Array>> {
    assertTransferTicket(ticket, "GET");
    if (
      expectedBytes !== undefined &&
      (!Number.isSafeInteger(expectedBytes) || expectedBytes < 0 || expectedBytes > ticket.max_bytes)
    ) {
      throw new SessionError("Aex file download has an invalid expected length");
    }
    const response = await this.#fetch(ticket.url, {
      method: "GET",
      redirect: "error",
      headers: ticket.headers,
      ...(signal === undefined ? {} : { signal }),
    });
    if (!response.ok) {
      throw new SessionError(`Aex file download returned HTTP ${response.status}`, {
        status: response.status,
      });
    }
    const declaredHeader = response.headers.get("content-length");
    const declared = declaredHeader === null ? undefined : Number(declaredHeader);
    if (declared !== undefined && Number.isFinite(declared) && declared > ticket.max_bytes) {
      throw new SessionError("Aex file download exceeded its transfer ticket");
    }
    if (
      expectedBytes !== undefined &&
      declared !== undefined &&
      Number.isFinite(declared) &&
      declared !== expectedBytes
    ) {
      throw new SessionError("Aex file download length does not match its object metadata");
    }
    return boundedStream(
      response.body ?? emptyStream(),
      ticket.max_bytes,
      signal,
      "download",
      expectedBytes,
    );
  }

  async uploadTransfer(
    ticket: TransferTicket,
    content: Uint8Array | (() => ReadableStream<Uint8Array>),
    bytes: number,
    signal?: AbortSignal,
  ): Promise<void> {
    assertTransferTicket(ticket, "PUT");
    if (!Number.isSafeInteger(bytes) || bytes < 0 || bytes > ticket.max_bytes) {
      throw new TypeError("Aex file upload exceeds its transfer ticket");
    }
    const streaming = typeof content === "function";
    const body = streaming
      ? exactStream(content(), bytes, ticket.max_bytes, signal)
      : arrayBufferBody(content);
    if (!streaming && content.byteLength !== bytes) {
      throw new TypeError("Aex file upload length does not match its transfer request");
    }
    const init: RequestInit & { duplex?: "half" } = {
      method: "PUT",
      redirect: "error",
      headers: ticket.headers,
      body,
      ...(signal === undefined ? {} : { signal }),
      ...(streaming ? { duplex: "half" as const } : {}),
    };
    const response = await this.#fetch(ticket.url, init);
    if (!response.ok) {
      throw new SessionError(`Aex file upload returned HTTP ${response.status}`, {
        status: response.status,
      });
    }
  }

  async json<T>(
    method: "GET" | "POST" | "DELETE",
    path: string,
    options: JsonRequestOptions = {},
  ): Promise<T> {
    const attempts = options.retry === true ? 2 : 1;
    // Serialize once before the first await. A caller mutating its source object after a lost
    // response must never send different bytes under the same idempotency identity on retry.
    const requestBody = options.body === undefined
      ? undefined
      : encodeJsonOnce(options.body, requestLimit(method, path), "Aex API request");
    let lastError: unknown;
    for (let attempt = 0; attempt < attempts; attempt += 1) {
      try {
        const response = await this.#fetch(`${this.baseUrl}${path}`, {
          method,
          redirect: "error",
          headers: {
            Accept: "application/json",
            ...(requestBody === undefined ? {} : { "Content-Type": "application/json" }),
            Authorization: `Bearer ${this.#apiKey}`,
            ...options.headers,
          },
          ...(requestBody === undefined ? {} : { body: requestBody }),
          ...(options.signal === undefined ? {} : { signal: options.signal }),
        });
        if (!response.ok) throw await this.responseError(response);
        if (response.status === 204) return undefined as T;
        const text = await readResponseText(response, MAX_ORDINARY_JSON_BYTES);
        if (text === "") return undefined as T;
        try {
          return JSON.parse(text) as T;
        } catch (cause) {
          throw new SessionError("Aex returned invalid JSON", { cause });
        }
      } catch (error) {
        if (options.signal?.aborted === true || isAbort(error)) throw abortError(error);
        if (error instanceof AexError) {
          const retryableServerFailure =
            options.retry === true && error.status !== undefined && error.status >= 500;
          if (!retryableServerFailure || attempt + 1 >= attempts) throw error;
        }
        lastError = error;
      }
    }
    throw new SessionError("Could not reach the Aex API", { cause: lastError });
  }

  /** Accept one durable deletion job; strict callers poll its short status resource client-side. */
  async deleteSession(
    sessionId: string,
    waitForCompletion: boolean,
    signal?: AbortSignal,
  ): Promise<void> {
    const path = `/v1/sessions/${encodeURIComponent(sessionId)}`;
    let accepted: Response | undefined;
    let lastError: unknown;
    for (let attempt = 0; attempt < 2; attempt += 1) {
      try {
        const response = await this.#fetch(`${this.baseUrl}${path}`, {
          method: "DELETE",
          redirect: "error",
          headers: {
            Accept: "application/json",
            Authorization: `Bearer ${this.#apiKey}`,
          },
          ...(signal === undefined ? {} : { signal }),
        });
        if (!response.ok) throw await this.responseError(response);
        if (response.status !== 202 && response.status !== 204) {
          throw new SessionError(`Aex returned invalid deletion status HTTP ${response.status}`);
        }
        accepted = response;
        break;
      } catch (error) {
        if (signal?.aborted === true || isAbort(error)) throw abortError(error);
        if (error instanceof AexError) {
          const retryableServerFailure = error.status !== undefined && error.status >= 500;
          if (!retryableServerFailure || attempt + 1 >= 2) throw error;
        }
        lastError = error;
      }
    }
    if (accepted === undefined) {
      throw new SessionError("Could not reach the Aex API", { cause: lastError });
    }
    if (accepted.status === 204 || !waitForCompletion) return;

    const canonicalStatusPath = `${path}/deletion`;
    const location = accepted.headers.get("location") ?? canonicalStatusPath;
    const statusUrl = deletionStatusUrl(this.baseUrl, canonicalStatusPath, location);
    let retryMs = retryAfterMs(accepted.headers.get("retry-after"));
    while (true) {
      await delay(jitter(retryMs), signal);
      let response: Response;
      try {
        response = await this.#fetch(statusUrl, {
          method: "GET",
          redirect: "error",
          headers: {
            Accept: "application/json",
            Authorization: `Bearer ${this.#apiKey}`,
          },
          ...(signal === undefined ? {} : { signal }),
        });
      } catch (error) {
        if (signal?.aborted === true || isAbort(error)) throw abortError(error);
        retryMs = Math.min(30_000, Math.max(250, retryMs * 2));
        continue;
      }
      if (!response.ok) {
        const error = await this.responseError(response);
        if (error.status === undefined || error.status < 500) throw error;
        retryMs = Math.min(
          30_000,
          Math.max(retryAfterMs(response.headers.get("retry-after")), retryMs * 2),
        );
        continue;
      }
      let status: unknown;
      try {
        const body = await readResponseText(response, MAX_ERROR_RESPONSE_BYTES);
        status = JSON.parse(body) as unknown;
      } catch (cause) {
        throw new SessionError("Aex returned invalid deletion status JSON", { cause });
      }
      const state = typeof status === "object" && status !== null && "state" in status
        ? (status as { state?: unknown }).state
        : undefined;
      if (state === "succeeded") return;
      if (
        state !== "accepted" &&
        state !== "deleting" &&
        state !== "retrying" &&
        state !== "blocked"
      ) {
        throw new SessionError("Aex returned an invalid deletion state");
      }
      retryMs = retryAfterMs(response.headers.get("retry-after"));
    }
  }

  async *events(sessionId: string, options: EventOptions = {}): AsyncGenerator<Event> {
    let cursor = options.after ?? 0;
    const follow = options.follow ?? true;
    let consecutiveFailures = 0;

    while (true) {
      try {
        const query = new URLSearchParams({ after: String(cursor), follow: String(follow) });
        const response = await this.#fetch(
          `${this.baseUrl}/v1/sessions/${encodeURIComponent(sessionId)}/events?${query}`,
          {
            method: "GET",
            redirect: "error",
            headers: {
              Accept: "text/event-stream",
              Authorization: `Bearer ${this.#apiKey}`,
              ...(cursor > 0 ? { "Last-Event-ID": String(cursor) } : {}),
            },
            ...(options.signal === undefined ? {} : { signal: options.signal }),
          },
        );
        if (!response.ok) throw await this.responseError(response);
        if (response.body === null) throw new SessionError("The Aex event stream had no body");

        let received = false;
        for await (const frame of parseEventFrames(response.body)) {
          // Only Brain's SSE `id` is reconnect authority. Provisional provider/Tool frames can
          // carry an internal JSON sequence while deliberately omitting `id`; advancing from the
          // payload would skip durable recovery records after a crash.
          if (frame.durableId !== undefined) {
            if (frame.durableId <= cursor) continue;
            cursor = frame.durableId;
          }
          received = true;
          consecutiveFailures = 0;
          yield frame.event;
        }
        if (!follow) return;
        if (!received) consecutiveFailures += 1;
      } catch (error) {
        if (options.signal?.aborted === true || isAbort(error)) throw abortError(error);
        if (error instanceof AexError && error.status !== undefined && error.status < 500) throw error;
        consecutiveFailures += 1;
        if (consecutiveFailures > 5) {
          if (error instanceof AexError) throw error;
          throw new SessionError("The Aex event stream disconnected", { cause: error });
        }
      }

      await delay(Math.min(2_000, 100 * 2 ** (consecutiveFailures - 1)), options.signal);
    }
  }

  private async responseError(response: Response): Promise<AexError> {
    let envelope: ErrorEnvelope | undefined;
    try {
      const text = await readResponseText(response, MAX_ERROR_RESPONSE_BYTES);
      envelope = text === "" ? undefined : JSON.parse(text) as ErrorEnvelope;
    } catch {
      // Fall through to a status-based error.
    }
    if (envelope?.error !== undefined) return errorFromApi(envelope.error, response.status);
    return new SessionError(`Aex API request failed with HTTP ${response.status}`, {
      status: response.status,
      requestId: response.headers.get("x-request-id") ?? undefined,
    });
  }
}

function requestLimit(method: "GET" | "POST" | "DELETE", path: string): number {
  if (method === "POST" && path === "/v1/sessions") return MAX_CREATE_SESSION_REQUEST_BYTES;
  if (
    method === "POST" &&
    (/^\/v1\/sessions\/[^/]+\/messages(?:\?|$)/u.test(path) ||
      /^\/v1\/sessions\/[^/]+\/children(?:\?|$)/u.test(path) ||
      /^\/v1\/sessions\/[^/]+\/children\/[^/]+\/(?:messages|follow-up)(?:\?|$)/u.test(path))
  ) {
    return MAX_MESSAGE_REQUEST_BYTES;
  }
  return MAX_ORDINARY_JSON_BYTES;
}

function encodeJsonOnce(value: unknown, maxBytes: number, label: string): string {
  let encoded: string | undefined;
  try {
    encoded = JSON.stringify(value);
  } catch (cause) {
    throw new TypeError(`${label} is not JSON-serializable`, { cause });
  }
  if (encoded === undefined) throw new TypeError(`${label} must be a JSON value`);
  if (new TextEncoder().encode(encoded).byteLength > maxBytes) {
    throw new TypeError(`${label} exceeds ${maxBytes} bytes`);
  }
  return encoded;
}

async function readResponseText(response: Response, maxBytes: number): Promise<string> {
  if (response.body === null) return "";
  const declared = Number(response.headers.get("content-length"));
  if (Number.isFinite(declared) && declared > maxBytes) {
    await response.body.cancel().catch(() => undefined);
    throw new SessionError(`Aex response exceeds ${maxBytes} bytes`);
  }
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let bytes = 0;
  try {
    while (true) {
      const item = await reader.read();
      if (item.done) break;
      bytes += item.value.byteLength;
      if (bytes > maxBytes) throw new SessionError(`Aex response exceeds ${maxBytes} bytes`);
      chunks.push(item.value);
    }
  } finally {
    await reader.cancel().catch(() => undefined);
    reader.releaseLock();
  }
  const content = new Uint8Array(bytes);
  let offset = 0;
  for (const chunk of chunks) {
    content.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return new TextDecoder().decode(content);
}

function assertTransferTicket(ticket: TransferTicket, method: "GET" | "PUT"): void {
  if (ticket.method !== method) {
    throw new SessionError(`Aex returned a ${ticket.method} transfer for ${method}`);
  }
  if (!Number.isSafeInteger(ticket.max_bytes) || ticket.max_bytes < 0) {
    throw new SessionError("Aex returned an invalid file-transfer limit");
  }
  let url: URL;
  try {
    url = new URL(ticket.url);
  } catch (cause) {
    throw new SessionError("Aex returned an invalid file-transfer URL", { cause });
  }
  if (url.protocol !== "https:" && !(url.protocol === "http:" && isLoopback(url.hostname))) {
    throw new SessionError("Aex file transfers require HTTPS (or loopback HTTP for development)");
  }
}

function validateCustomerHandGrant(
  baseUrl: string,
  grant: {
    url: string;
    protocol: string;
    expires_at: string;
    grant_id: string;
    observation_url: string;
    observation_token: string;
  },
): string {
  if (!/^[A-Za-z0-9_.-]{1,128}$/u.test(grant.grant_id)) {
    throw new SessionError("Aex returned an invalid customer Hand grant id");
  }
  if (!/^[!#$%&'*+\-.^_`|~0-9A-Za-z]{1,2048}$/u.test(grant.protocol)) {
    throw new SessionError("Aex returned an invalid customer Hand WebSocket protocol");
  }
  if (!/^[\x21-\x7e]{1,2048}$/u.test(grant.observation_token)) {
    throw new SessionError("Aex returned an invalid customer Hand observation token");
  }

  let socket: URL;
  let observation: URL;
  let expectedObservation: URL;
  try {
    socket = new URL(grant.url);
    observation = new URL(grant.observation_url);
    expectedObservation = new URL(
      `${baseUrl}/v1/customer-hand/observations/${encodeURIComponent(grant.grant_id)}`,
    );
  } catch (cause) {
    throw new SessionError("Aex returned an invalid customer Hand URL", { cause });
  }
  const secureSocket = socket.protocol === "wss:" ||
    (socket.protocol === "ws:" && isLoopback(socket.hostname));
  if (
    !secureSocket || socket.username !== "" || socket.password !== "" ||
    socket.search !== "" || socket.hash !== ""
  ) {
    throw new SessionError(
      "Aex customer Hand sockets require credential-free WSS (or loopback WS for development)",
    );
  }
  if (observation.href !== expectedObservation.href) {
    throw new SessionError("Aex returned an unsafe customer Hand observation URL");
  }
  if (observation.href.includes(grant.observation_token)) {
    throw new SessionError("Aex returned an observation URL containing its bearer token");
  }
  return expectedObservation.href;
}

function arrayBufferBody(content: Uint8Array): Uint8Array<ArrayBuffer> {
  if (content.buffer instanceof ArrayBuffer) {
    return content as Uint8Array<ArrayBuffer>;
  }
  // SharedArrayBuffer-backed views are not a Fetch BodyInit. This uncommon compatibility copy is
  // the only buffered-copy path; ordinary Uint8Array/ArrayBuffer uploads stay zero-copy.
  return Uint8Array.from(content);
}

function emptyStream(): ReadableStream<Uint8Array> {
  return new ReadableStream<Uint8Array>({
    start(controller) {
      controller.close();
    },
  });
}

function boundedStream(
  source: ReadableStream<Uint8Array>,
  maxBytes: number,
  signal: AbortSignal | undefined,
  direction: "upload" | "download",
  expectedBytes?: number,
): ReadableStream<Uint8Array> {
  const reader = source.getReader();
  let bytes = 0;
  let finished = false;
  let controller: ReadableStreamDefaultController<Uint8Array> | undefined;
  const cleanup = (): void => signal?.removeEventListener("abort", onAbort);
  const onAbort = (): void => {
    if (finished) return;
    finished = true;
    void reader.cancel(signal?.reason).catch(() => undefined);
    controller?.error(abortError(signal?.reason));
    cleanup();
  };
  return new ReadableStream<Uint8Array>({
    start(value) {
      controller = value;
      if (signal?.aborted === true) onAbort();
      else signal?.addEventListener("abort", onAbort, { once: true });
    },
    async pull(value) {
      if (finished) return;
      try {
        const item = await reader.read();
        if (item.done) {
          if (expectedBytes !== undefined && bytes !== expectedBytes) {
            throw new TypeError(
              `Aex file ${direction} produced ${bytes} bytes; expected ${expectedBytes}`,
            );
          }
          finished = true;
          cleanup();
          value.close();
          return;
        }
        if (!(item.value instanceof Uint8Array)) {
          throw new TypeError(`Aex file ${direction} stream must yield Uint8Array chunks`);
        }
        bytes += item.value.byteLength;
        if (bytes > maxBytes || (expectedBytes !== undefined && bytes > expectedBytes)) {
          throw new TypeError(`Aex file ${direction} exceeded its declared byte limit`);
        }
        value.enqueue(item.value);
      } catch (error) {
        finished = true;
        cleanup();
        await reader.cancel(error).catch(() => undefined);
        value.error(error);
      }
    },
    async cancel(reason) {
      if (finished) return;
      finished = true;
      cleanup();
      await reader.cancel(reason);
    },
  });
}

function exactStream(
  source: ReadableStream<Uint8Array>,
  expectedBytes: number,
  maxBytes: number,
  signal?: AbortSignal,
): ReadableStream<Uint8Array> {
  return boundedStream(source, maxBytes, signal, "upload", expectedBytes);
}

function isLoopback(hostname: string): boolean {
  return hostname === "localhost" || hostname === "127.0.0.1" || hostname === "[::1]";
}

function deletionStatusUrl(baseUrl: string, canonicalPath: string, location: string): string {
  let url: URL;
  try {
    url = new URL(location, `${baseUrl}/`);
  } catch (cause) {
    throw new SessionError("Aex returned an invalid deletion status location", { cause });
  }
  const base = new URL(`${baseUrl}/`);
  if (url.origin !== base.origin || url.pathname !== canonicalPath || url.search !== "") {
    throw new SessionError("Aex returned an unsafe deletion status location");
  }
  return url.toString();
}

function retryAfterMs(value: string | null): number {
  if (value !== null && /^\d+$/u.test(value)) {
    return Math.min(30_000, Math.max(25, Number(value) * 1_000));
  }
  if (value !== null) {
    const at = Date.parse(value);
    if (Number.isFinite(at)) return Math.min(30_000, Math.max(25, at - Date.now()));
  }
  return 1_000;
}

function jitter(milliseconds: number): number {
  return Math.floor(milliseconds / 2 + Math.random() * (milliseconds / 2));
}

const MAX_SSE_FRAMING_BYTES = 4 * 1024;
const MAX_SSE_WIRE_FRAME_BYTES = MAX_PUBLIC_EVENT_BYTES + MAX_SSE_FRAMING_BYTES;
const MAX_SSE_LINE_BYTES = MAX_SSE_WIRE_FRAME_BYTES;

/** @internal Incremental, constant-space decoder for Brain's bounded public SSE events. */
export async function* parseEventStream(
  stream: ReadableStream<Uint8Array>,
): AsyncGenerator<Event> {
  for await (const frame of parseEventFrames(stream)) yield frame.event;
}

interface ParsedEventFrame {
  event: Event;
  durableId?: number;
}

async function* parseEventFrames(
  stream: ReadableStream<Uint8Array>,
): AsyncGenerator<ParsedEventFrame> {
  const reader = stream.getReader();
  const decoder = new TextDecoder("utf-8", { fatal: true });
  const line = new Uint8Array(MAX_SSE_LINE_BYTES);
  const data = new Uint8Array(MAX_PUBLIC_EVENT_BYTES);
  let lineLength = 0;
  let dataLength = 0;
  let dataLines = 0;
  let frameFramingBytes = 0;
  let frameStarted = false;
  let eventId: string | undefined;

  const reset = (): void => {
    dataLength = 0;
    dataLines = 0;
    frameFramingBytes = 0;
    frameStarted = false;
    eventId = undefined;
  };

  const dispatch = (): ParsedEventFrame | undefined => {
    if (dataLines === 0) {
      reset();
      return undefined;
    }
    let encoded: string;
    try {
      encoded = decoder.decode(data.subarray(0, dataLength));
    } catch (cause) {
      throw new SessionError("Aex sent an event that was not valid UTF-8", { cause });
    }
    const id = eventId;
    reset();
    try {
      const event = JSON.parse(encoded) as Event;
      if (id === undefined) {
        if (!isEphemeralEvent(event)) {
          throw new SessionError(`Aex sent durable event ${event.type} without an SSE id`);
        }
        return { event };
      }
      if (!/^[1-9][0-9]*$/u.test(id)) {
        throw new SessionError("Aex sent an invalid durable SSE id");
      }
      const durableId = Number(id);
      const payloadSeq = "seq" in event ? event.seq : undefined;
      if (!Number.isSafeInteger(durableId) || payloadSeq !== durableId) {
        throw new SessionError("Aex durable SSE id does not match its event sequence");
      }
      return { event, durableId };
    } catch (cause) {
      if (cause instanceof SessionError) throw cause;
      throw new SessionError("Aex sent an invalid event", { cause });
    }
  };

  const processLine = (): ParsedEventFrame | undefined => {
    const wireLineBytes = lineLength + 1;
    let end = lineLength;
    if (end > 0 && line[end - 1] === 13) end -= 1;
    lineLength = 0;
    const addFraming = (payloadBytes = 0): void => {
      frameFramingBytes += wireLineBytes - payloadBytes;
      if (frameFramingBytes > MAX_SSE_FRAMING_BYTES) {
        throw new SessionError(
          `Aex event frame exceeds ${MAX_SSE_FRAMING_BYTES} framing bytes`,
        );
      }
    };
    if (end === 0) {
      addFraming();
      return dispatch();
    }
    frameStarted = true;
    if (line[0] === 58) {
      addFraming();
      return undefined; // SSE comment.
    }

    let separator = -1;
    for (let index = 0; index < end; index += 1) {
      if (line[index] === 58) {
        separator = index;
        break;
      }
    }
    let start = separator + 1;
    if (start < end && line[start] === 32) start += 1;
    const isData = separator === 4 && line[0] === 100 && line[1] === 97 &&
      line[2] === 116 && line[3] === 97;
    const isId = separator === 2 && line[0] === 105 && line[1] === 100;
    if (!isData) addFraming();
    if (isId) {
      try {
        eventId = decoder.decode(line.subarray(start, end));
      } catch (cause) {
        throw new SessionError("Aex sent an SSE id that was not valid UTF-8", { cause });
      }
      return undefined;
    }
    if (!isData) return undefined;
    const contentBytes = end - start;
    addFraming(contentBytes);
    const joinedBytes = dataLength + (dataLines > 0 ? 1 : 0) + contentBytes;
    if (joinedBytes > MAX_PUBLIC_EVENT_BYTES) {
      throw new SessionError(`Aex event payload exceeds ${MAX_PUBLIC_EVENT_BYTES} bytes`);
    }
    if (dataLines > 0) data[dataLength++] = 10;
    data.set(line.subarray(start, end), dataLength);
    dataLength += contentBytes;
    dataLines += 1;
    return undefined;
  };

  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      if (!(value instanceof Uint8Array)) {
        throw new SessionError("Aex event stream yielded a non-byte chunk");
      }
      for (const byte of value) {
        if (byte === 10) {
          const event = processLine();
          if (event !== undefined) yield event;
          continue;
        }
        if (lineLength >= line.byteLength) {
          throw new SessionError(`Aex event line exceeds ${MAX_SSE_LINE_BYTES} bytes`);
        }
        line[lineLength++] = byte;
      }
    }
    if (lineLength > 0 || frameStarted || dataLines > 0) {
      throw new SessionError("Aex event stream ended in a truncated SSE frame");
    }
  } finally {
    await reader.cancel().catch(() => undefined);
    reader.releaseLock();
  }
}

function isEphemeralEvent(event: Event): boolean {
  return event.type === "assistant.delta" || event.type === "tool.output" ||
    event.type === "replay.complete";
}

function isAbort(error: unknown): boolean {
  return error instanceof AbortError || (error instanceof DOMException && error.name === "AbortError");
}

function delay(milliseconds: number, signal?: AbortSignal): Promise<void> {
  if (signal?.aborted === true) return Promise.reject(abortError(signal.reason));
  return new Promise((resolve, reject) => {
    const cleanup = (): void => signal?.removeEventListener("abort", onAbort);
    const timer = setTimeout(() => {
      cleanup();
      resolve();
    }, milliseconds);
    const onAbort = (): void => {
      clearTimeout(timer);
      cleanup();
      reject(abortError(signal?.reason));
    };
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}
