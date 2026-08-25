import {
  MAX_INLINE_FILE_BYTES,
  SessionChildren as BrainChildren,
  SessionSandbox as BrainSandbox,
  SessionStorage as BrainStorage,
} from "@aexhq/brain";
import type {
  SandboxFileEntry as BrainSandboxFileEntry,
  SandboxStatus as BrainSandboxStatus,
  StorageObject as BrainStorageObject,
} from "@aexhq/brain";
import type { Event, Session as BrainSession } from "@aexhq/brain/session";

import type { EventOptions } from "./transport.js";
import { Transport } from "./transport.js";
import { SessionError, abortError } from "./errors.js";
import { randomIdempotencyKey } from "./json.js";

export type BinarySource = string | ArrayBuffer | ArrayBufferView | Blob;

/** A replayable, length- and digest-declared source for O(1)-heap large uploads. */
export interface StreamingUploadSource {
  readonly bytes: number;
  readonly sha256: string;
  stream(): ReadableStream<Uint8Array>;
}

export type UploadSource = BinarySource | StreamingUploadSource;

export interface OperationOptions {
  signal?: AbortSignal;
}

export interface IdempotentOperationOptions extends OperationOptions {
  /** Stable retry identity; the SDK generates one when omitted. */
  idempotencyKey?: string;
}

export interface PageOptions extends OperationOptions {
  cursor?: string;
  limit?: number;
}

export interface SandboxStatus {
  state: BrainSandboxStatus["state"];
  generation?: string;
  reason?: string;
  changedAt?: string;
  expiresAt?: string;
}

export interface SandboxFile {
  path: string;
  kind: BrainSandboxFileEntry["kind"];
  bytes: number;
  sha256?: string;
  modifiedAt: string;
}

export interface SandboxFilePage {
  data: SandboxFile[];
  hasMore: boolean;
  nextCursor?: string;
  generation: string;
}

export interface SandboxFileOptions extends OperationOptions {
  generation: string;
}

export interface SandboxFilePageOptions extends SandboxFileOptions, PageOptions {}

export class SandboxFiles {
  readonly #inner: BrainSandbox["files"];
  readonly #transport: Transport;

  constructor(inner: BrainSandbox["files"], transport: Transport) {
    this.#inner = inner;
    this.#transport = transport;
  }

  async list(path: string, options: SandboxFilePageOptions): Promise<SandboxFilePage> {
    const page = await this.#inner.list(
      {
        path,
        generation: options.generation,
        ...(options.cursor === undefined ? {} : { cursor: options.cursor }),
        ...(options.limit === undefined ? {} : { limit: options.limit }),
      },
      request(options),
    );
    return filePage(page);
  }

  async stat(path: string, options: SandboxFileOptions): Promise<SandboxFile> {
    return file(await this.#inner.stat({ path, generation: options.generation }, request(options)));
  }

  async download(path: string, options: SandboxFileOptions): Promise<Uint8Array> {
    return collect(await this.downloadStream(path, options));
  }

  async downloadStream(
    path: string,
    options: SandboxFileOptions,
  ): Promise<ReadableStream<Uint8Array>> {
    const entry = await this.#inner.stat({ path, generation: options.generation }, request(options));
    if (entry.bytes <= MAX_INLINE_FILE_BYTES) {
      const result = await this.#inner.readInline(
        { path, generation: options.generation },
        request(options),
      );
      return byteStream(decodeBase64(result.content_base64));
    }
    const ticket = await this.#inner.prepareDownload(
      { path, generation: options.generation },
      request(options),
    );
    return this.#transport.downloadTransferStream(ticket, options.signal, entry.bytes);
  }

  async upload(
    path: string,
    source: UploadSource,
    options: SandboxFileOptions & { overwrite?: boolean },
  ): Promise<SandboxFile> {
    const base = {
      path,
      generation: options.generation,
      ...(options.overwrite === undefined ? {} : { overwrite: options.overwrite }),
    };
    let entry: BrainSandboxFileEntry;
    if (isStreamingSource(source)) {
      assertStreamingSource(source);
      entry = source.bytes <= MAX_INLINE_FILE_BYTES
        ? await this.#inner.writeInline(
            { ...base, content_base64: encodeBase64(await collectDeclared(source, options.signal)) },
            request(options),
          )
        : await this.#uploadStream(base, source, options);
    } else {
      const content = await bytesOf(source);
      entry = content.byteLength <= MAX_INLINE_FILE_BYTES
        ? await this.#inner.writeInline(
            { ...base, content_base64: encodeBase64(content) },
            request(options),
          )
        : await this.#upload(base, content, options);
    }
    return file(entry);
  }

  async #upload(
    input: { path: string; generation: string; overwrite?: boolean },
    content: Uint8Array,
    options: SandboxFileOptions,
  ): Promise<BrainSandboxFileEntry> {
    const ticket = await this.#inner.prepareUpload(
      { ...input, bytes: content.byteLength, sha256: await sha256(content) },
      request(options),
    );
    await this.#transport.uploadTransfer(ticket, content, content.byteLength, options.signal);
    // Direct sandbox transfers are intentionally happy-path only. Do not automatically replay an
    // ambiguous import: surface the error so the caller can inspect the generation/path and
    // prepare a fresh transfer. Durable storage completion has separate retry-safe semantics.
    return this.#inner.completeUpload(ticket.transfer_id, request(options));
  }

  async #uploadStream(
    input: { path: string; generation: string; overwrite?: boolean },
    source: StreamingUploadSource,
    options: SandboxFileOptions,
  ): Promise<BrainSandboxFileEntry> {
    const ticket = await this.#inner.prepareUpload(
      { ...input, bytes: source.bytes, sha256: source.sha256 },
      request(options),
    );
    await this.#transport.uploadTransfer(ticket, () => source.stream(), source.bytes, options.signal);
    return this.#inner.completeUpload(ticket.transfer_id, request(options));
  }

  async find(
    input: { path: string; glob: string },
    options: SandboxFilePageOptions,
  ): Promise<SandboxFilePage> {
    const page = await this.#inner.find(
      {
        ...input,
        generation: options.generation,
        ...(options.cursor === undefined ? {} : { cursor: options.cursor }),
        ...(options.limit === undefined ? {} : { limit: options.limit }),
      },
      request(options),
    );
    return filePage(page);
  }

  async grep(
    input: { path: string; query: string },
    options: SandboxFilePageOptions,
  ): Promise<SandboxFilePage> {
    const page = await this.#inner.grep(
      {
        ...input,
        generation: options.generation,
        ...(options.cursor === undefined ? {} : { cursor: options.cursor }),
        ...(options.limit === undefined ? {} : { limit: options.limit }),
      },
      request(options),
    );
    return filePage(page);
  }
}

export class SessionSandbox {
  readonly #inner: BrainSandbox;
  readonly files: SandboxFiles;

  constructor(transport: Transport, sessionId: string) {
    this.#inner = new BrainSandbox(transport, sessionId);
    this.files = new SandboxFiles(this.#inner.files, transport);
  }

  async status(options: OperationOptions = {}): Promise<SandboxStatus> {
    return sandboxStatus(await this.#inner.status(request(options)));
  }

  async create(options: OperationOptions = {}): Promise<SandboxStatus> {
    return sandboxStatus(await this.#inner.create(intrinsicRequest(options)));
  }
}

export interface StorageObject {
  key: string;
  bytes: number;
  sha256: string;
  contentType?: string;
  createdAt: string;
  updatedAt: string;
}

export interface StoragePage {
  data: StorageObject[];
  hasMore: boolean;
  nextCursor?: string;
}

export class SessionStorage {
  readonly #inner: BrainStorage;
  readonly #transport: Transport;

  constructor(transport: Transport, sessionId: string) {
    this.#inner = new BrainStorage(transport, sessionId);
    this.#transport = transport;
  }

  async list(options: PageOptions & { prefix?: string } = {}): Promise<StoragePage> {
    const page = await this.#inner.list(
      {
        ...(options.prefix === undefined ? {} : { prefix: options.prefix }),
        ...(options.cursor === undefined ? {} : { cursor: options.cursor }),
        ...(options.limit === undefined ? {} : { limit: options.limit }),
      },
      request(options),
    );
    return {
      data: page.data.map(storageObject),
      hasMore: page.has_more,
      ...(page.next_cursor === undefined ? {} : { nextCursor: page.next_cursor }),
    };
  }

  async stat(key: string, options: OperationOptions = {}): Promise<StorageObject> {
    return storageObject(await this.#inner.stat(key, request(options)));
  }

  async download(key: string, options: OperationOptions = {}): Promise<Uint8Array> {
    return collect(await this.downloadStream(key, options));
  }

  async downloadStream(
    key: string,
    options: OperationOptions = {},
  ): Promise<ReadableStream<Uint8Array>> {
    const object = await this.#inner.stat(key, request(options));
    if (object.bytes <= MAX_INLINE_FILE_BYTES) {
      return byteStream(decodeBase64(
        (await this.#inner.readInline(key, request(options))).content_base64,
      ));
    }
    const ticket = await this.#inner.prepareDownload(key, request(options));
    return this.#transport.downloadTransferStream(ticket, options.signal, object.bytes);
  }

  async upload(
    key: string,
    source: UploadSource,
    options: OperationOptions & { contentType?: string; overwrite?: boolean } = {},
  ): Promise<StorageObject> {
    const base = {
      key,
      ...(options.contentType === undefined ? {} : { content_type: options.contentType }),
      ...(options.overwrite === undefined ? {} : { overwrite: options.overwrite }),
    };
    if (isStreamingSource(source)) {
      assertStreamingSource(source);
      if (source.bytes <= MAX_INLINE_FILE_BYTES) {
        return storageObject(await this.#inner.writeInline(
          { ...base, content_base64: encodeBase64(await collectDeclared(source, options.signal)) },
          request(options),
        ));
      }
      const ticket = await this.#inner.prepareUpload(
        { ...base, bytes: source.bytes, sha256: source.sha256 },
        request(options),
      );
      await this.#transport.uploadTransfer(ticket, () => source.stream(), source.bytes, options.signal);
      return storageObject(await this.#inner.completeUpload(ticket.transfer_id, intrinsicRequest(options)));
    }
    const content = await bytesOf(source);
    if (content.byteLength <= MAX_INLINE_FILE_BYTES) {
      return storageObject(await this.#inner.writeInline(
        { ...base, content_base64: encodeBase64(content) },
        request(options),
      ));
    }
    const ticket = await this.#inner.prepareUpload(
      { ...base, bytes: content.byteLength, sha256: await sha256(content) },
      request(options),
    );
    await this.#transport.uploadTransfer(ticket, content, content.byteLength, options.signal);
    return storageObject(await this.#inner.completeUpload(ticket.transfer_id, intrinsicRequest(options)));
  }

  delete(key: string, options: OperationOptions = {}): Promise<void> {
    return this.#inner.delete(key, request(options));
  }

  async copyFromSandbox(
    input: { environment: string; key: string; path: string; sandboxGeneration: string; overwrite?: boolean },
    options: OperationOptions = {},
  ): Promise<StorageObject> {
    return storageObject(await this.#inner.copyFromEnvironment(
      input.environment,
      {
        key: input.key,
        path: input.path,
        environment_generation: input.sandboxGeneration,
        ...(input.overwrite === undefined ? {} : { overwrite: input.overwrite }),
      },
      request(options),
    ));
  }

  async copyToSandbox(
    input: { environment: string; key: string; path: string; sandboxGeneration: string; overwrite?: boolean },
    options: OperationOptions = {},
  ): Promise<SandboxFile> {
    return file(await this.#inner.copyToEnvironment(
      input.environment,
      {
        key: input.key,
        path: input.path,
        environment_generation: input.sandboxGeneration,
        ...(input.overwrite === undefined ? {} : { overwrite: input.overwrite }),
      },
      request(options),
    ));
  }
}

export interface ChildSummary {
  id: string;
  parentId: string;
  rootId: string;
  name?: string;
  depth: number;
  state: BrainSession["state"];
  turnState: BrainSession["turn_state"];
  turnPhase?: string;
  shape: BrainSession["shape"];
  createdAt: string;
  updatedAt: string;
}

export class SessionChild {
  readonly #inner: ReturnType<BrainChildren["get"]>;

  constructor(inner: ReturnType<BrainChildren["get"]>) {
    this.#inner = inner;
  }

  get id(): string {
    return this.#inner.id;
  }

  async info(options: OperationOptions = {}): Promise<ChildSummary> {
    return child(await this.#inner.get(request(options)));
  }

  async send(message: string, options: IdempotentOperationOptions = {}): Promise<void> {
    await this.#inner.send(message, idempotentRequest(options));
  }

  async followUp(
    message: string,
    options: IdempotentOperationOptions = {},
  ): Promise<ChildSummary> {
    return child(await this.#inner.followUp(message, idempotentRequest(options)));
  }

  async wait(options: OperationOptions & { timeoutMs?: number } = {}): Promise<ChildSummary> {
    return child(await this.#inner.wait(
      options.timeoutMs === undefined ? {} : { timeout_ms: options.timeoutMs },
      request(options),
    ));
  }

  async interrupt(options: OperationOptions = {}): Promise<ChildSummary> {
    return child(await this.#inner.interrupt(intrinsicRequest(options)));
  }

  async end(options: OperationOptions = {}): Promise<ChildSummary> {
    return child(await this.#inner.end(intrinsicRequest(options)));
  }

  events(options: EventOptions = {}): AsyncGenerator<Event> {
    return this.#inner.events(options);
  }
}

export class SessionChildren {
  readonly #inner: BrainChildren;

  constructor(transport: Transport, sessionId: string) {
    this.#inner = new BrainChildren(transport, sessionId);
  }

  async create(
    input: { prompt: string; name?: string; forkTurns?: "all" | "none" | `${number}` },
    options: IdempotentOperationOptions = {},
  ): Promise<SessionChild> {
    const handle = await this.#inner.create(
      {
        prompt: input.prompt,
        ...(input.name === undefined ? {} : { name: input.name }),
        ...(input.forkTurns === undefined ? {} : { fork_turns: input.forkTurns }),
      },
      idempotentRequest(options),
    );
    return new SessionChild(handle);
  }

  async list(options: PageOptions = {}): Promise<{
    data: ChildSummary[];
    hasMore: boolean;
    nextCursor?: string;
  }> {
    const page = await this.#inner.list(
      {
        ...(options.cursor === undefined ? {} : { cursor: options.cursor }),
        ...(options.limit === undefined ? {} : { limit: options.limit }),
      },
      request(options),
    );
    return {
      data: page.data.map(child),
      hasMore: page.has_more,
      ...(page.next_cursor === undefined ? {} : { nextCursor: page.next_cursor }),
    };
  }

  get(childId: string): SessionChild {
    return new SessionChild(this.#inner.get(childId));
  }
}

function request(options: OperationOptions): { signal?: AbortSignal } {
  return options.signal === undefined ? {} : { signal: options.signal };
}

function intrinsicRequest(options: OperationOptions): { signal?: AbortSignal; retry: true } {
  return {
    ...(options.signal === undefined ? {} : { signal: options.signal }),
    retry: true,
  };
}

function idempotentRequest(options: IdempotentOperationOptions): {
  signal?: AbortSignal;
  headers: { "Idempotency-Key": string };
  retry: true;
} {
  return {
    ...(options.signal === undefined ? {} : { signal: options.signal }),
    headers: { "Idempotency-Key": options.idempotencyKey ?? randomIdempotencyKey() },
    retry: true,
  };
}

function sandboxStatus(value: BrainSandboxStatus): SandboxStatus {
  return {
    state: value.state,
    ...(value.generation == null ? {} : { generation: value.generation }),
    ...(value.reason == null ? {} : { reason: value.reason }),
    ...(value.changed_at_ms == null ? {} : { changedAt: timestamp(value.changed_at_ms) }),
    ...(value.expires_at_ms == null ? {} : { expiresAt: timestamp(value.expires_at_ms) }),
  };
}

function file(value: BrainSandboxFileEntry): SandboxFile {
  return {
    path: value.path,
    kind: value.kind,
    bytes: value.bytes,
    ...(value.sha256 == null ? {} : { sha256: value.sha256 }),
    modifiedAt: timestamp(value.modified_at_ms),
  };
}

function filePage(value: Awaited<ReturnType<BrainSandbox["files"]["list"]>>): SandboxFilePage {
  return {
    data: value.data.map(file),
    hasMore: value.has_more,
    ...(value.next_cursor === undefined ? {} : { nextCursor: value.next_cursor }),
    generation: value.generation,
  };
}

function storageObject(value: BrainStorageObject): StorageObject {
  return {
    key: value.key,
    bytes: value.bytes,
    sha256: value.sha256,
    ...(value.content_type === undefined ? {} : { contentType: value.content_type }),
    createdAt: value.created_at,
    updatedAt: value.updated_at,
  };
}

function child(value: BrainSession): ChildSummary {
  if (value.parent_id === undefined) {
    throw new SessionError(`Aex returned root session ${value.id} from a direct-child resource`);
  }
  return {
    id: value.id,
    parentId: value.parent_id,
    rootId: value.root_id,
    ...(value.name === undefined ? {} : { name: value.name }),
    depth: value.depth,
    state: value.state,
    turnState: value.turn_state,
    ...(value.turn_phase === undefined ? {} : { turnPhase: value.turn_phase }),
    shape: value.shape,
    createdAt: value.created_at,
    updatedAt: value.updated_at,
  };
}

function timestamp(value: number): string {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new SessionError("Aex returned an invalid sandbox timestamp");
  }
  try {
    return new Date(value).toISOString();
  } catch (cause) {
    throw new SessionError("Aex returned an invalid sandbox timestamp", { cause });
  }
}

async function bytesOf(source: BinarySource): Promise<Uint8Array> {
  if (typeof source === "string") return new TextEncoder().encode(source);
  if (source instanceof ArrayBuffer) return new Uint8Array(source);
  if (typeof Blob !== "undefined" && source instanceof Blob) {
    return new Uint8Array(await source.arrayBuffer());
  }
  if (ArrayBuffer.isView(source)) {
    return new Uint8Array(source.buffer, source.byteOffset, source.byteLength);
  }
  throw new TypeError("Unsupported binary source");
}

function isStreamingSource(source: UploadSource): source is StreamingUploadSource {
  return typeof source === "object" && source !== null && "stream" in source &&
    "bytes" in source && "sha256" in source && !(typeof Blob !== "undefined" && source instanceof Blob);
}

function assertStreamingSource(source: StreamingUploadSource): void {
  if (!Number.isSafeInteger(source.bytes) || source.bytes < 0) {
    throw new TypeError("Streaming upload bytes must be a non-negative safe integer");
  }
  if (!/^[0-9a-f]{64}$/u.test(source.sha256)) {
    throw new TypeError("Streaming upload sha256 must be 64 lowercase hexadecimal characters");
  }
  if (typeof source.stream !== "function") {
    throw new TypeError("Streaming upload source must provide stream()");
  }
}

function byteStream(bytes: Uint8Array): ReadableStream<Uint8Array> {
  return new ReadableStream<Uint8Array>({
    start(controller) {
      if (bytes.byteLength !== 0) controller.enqueue(bytes);
      controller.close();
    },
  });
}

async function collect(stream: ReadableStream<Uint8Array>): Promise<Uint8Array> {
  const chunks: Uint8Array[] = [];
  let bytes = 0;
  const reader = stream.getReader();
  try {
    while (true) {
      const item = await reader.read();
      if (item.done) break;
      chunks.push(item.value);
      bytes += item.value.byteLength;
    }
  } finally {
    reader.releaseLock();
  }
  const result = new Uint8Array(bytes);
  let offset = 0;
  for (const chunk of chunks) {
    result.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return result;
}

async function collectDeclared(
  source: StreamingUploadSource,
  signal?: AbortSignal,
): Promise<Uint8Array> {
  if (signal?.aborted === true) throw abortError(signal.reason);
  const reader = source.stream().getReader();
  const result = new Uint8Array(source.bytes);
  let offset = 0;
  let aborted = false;
  const abort = (): void => {
    aborted = true;
    void reader.cancel(signal?.reason).catch(() => undefined);
  };
  signal?.addEventListener("abort", abort, { once: true });
  try {
    while (true) {
      const item = await reader.read();
      if (aborted) throw abortError(signal?.reason);
      if (item.done) break;
      if (!(item.value instanceof Uint8Array) || offset + item.value.byteLength > result.byteLength) {
        throw new TypeError("Streaming upload exceeded its declared byte length");
      }
      result.set(item.value, offset);
      offset += item.value.byteLength;
    }
  } finally {
    signal?.removeEventListener("abort", abort);
    reader.releaseLock();
  }
  if (offset !== result.byteLength) {
    throw new TypeError(`Streaming upload produced ${offset} bytes; expected ${result.byteLength}`);
  }
  if (await sha256(result) !== source.sha256) {
    throw new TypeError("Streaming upload content does not match its declared sha256");
  }
  return result;
}

export function encodeBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let offset = 0; offset < bytes.byteLength; offset += 32_768) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 32_768));
  }
  return btoa(binary);
}

function decodeBase64(value: string): Uint8Array {
  const binary = atob(value);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

async function sha256(value: Uint8Array): Promise<string> {
  const input = value.buffer instanceof ArrayBuffer
    ? value as Uint8Array<ArrayBuffer>
    : Uint8Array.from(value);
  const digest = await crypto.subtle.digest("SHA-256", input);
  return [...new Uint8Array(digest)]
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}
