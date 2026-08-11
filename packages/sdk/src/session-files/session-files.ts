import { createHash, randomUUID } from "node:crypto";
import { link, open, rename, unlink, type FileHandle } from "node:fs/promises";
import { basename, dirname, join } from "node:path";

import type { ResourceExecutor } from "../generated/resources.js";
import { AexConfigError } from "../transport/errors.js";

export const LIVE_FILE_PART_BYTES = 4_194_304;
export const MAX_LIVE_FILE_BYTES = 5_368_709_120;
export const DEFAULT_LIVE_FILE_CONCURRENCY = 4;
export const MAX_LIVE_FILE_CONCURRENCY = 8;

export interface LiveFilePart {
  readonly partNumber: number;
  readonly offset: string;
  readonly sizeBytes: number;
  readonly sha256: string;
}

export interface WorkspaceAccess {
  readonly generationId: string;
  readonly resumed: boolean;
}

export interface LiveFileUpload {
  readonly id: string;
  readonly sessionId: string;
  readonly generationId: string;
  readonly path: string;
  readonly state: "staging" | "complete";
  readonly sizeBytes: string;
  readonly sha256: string;
  readonly mode: "0644" | "0755";
  readonly partSizeBytes: number;
  readonly partCount: number;
  readonly parts: readonly LiveFilePart[];
  readonly expiresAt: string;
  readonly workspaceAccess: WorkspaceAccess;
}

export interface LiveFileDownload {
  readonly id: string;
  readonly sessionId: string;
  readonly generationId: string;
  readonly path: string;
  readonly state: "open" | "verified";
  readonly sizeBytes: string;
  readonly sha256: string;
  readonly version: string;
  readonly partSizeBytes: number;
  readonly partCount: number;
  readonly parts: readonly LiveFilePart[];
  readonly expiresAt: string;
  readonly workspaceAccess: WorkspaceAccess;
}

export interface UploadLiveFileOptions {
  readonly sessionId: string;
  readonly path: string;
  readonly sourcePath: string;
  readonly mode?: "0644" | "0755";
  readonly ifGenerationId?: string;
  /** Stable across retries of this upload. A random key is used when omitted. */
  readonly idempotencyKey?: string;
  readonly concurrency?: number;
}

export interface DownloadLiveFileOptions {
  readonly sessionId: string;
  readonly path: string;
  readonly destinationPath: string;
  readonly ifGenerationId?: string;
  /** Stable across retries of this download. A random key is used when omitted. */
  readonly idempotencyKey?: string;
  readonly concurrency?: number;
  /** Replace the destination atomically. The default refuses an existing path. */
  readonly replace?: boolean;
}

/**
 * Bounded Node.js convenience flows for generation-local session files.
 *
 * Public parts are four MiB, but no whole file is buffered: at most
 * `concurrency` part buffers exist. Uploads can resume from verified server
 * receipts. Downloads remain in an adjacent temporary file until every part,
 * the whole digest, and the server's final exact-version re-stat succeed.
 */
export class SessionFiles {
  readonly #executor: ResourceExecutor;

  constructor(executor: ResourceExecutor) {
    this.#executor = executor;
  }

  async uploadFile(options: UploadLiveFileOptions): Promise<LiveFileUpload> {
    const concurrency = boundedConcurrency(options.concurrency);
    const handle = await open(options.sourcePath, "r");
    try {
      const before = await handle.stat();
      if (!before.isFile()) throw new AexConfigError("live upload source must be a regular file");
      if (!Number.isSafeInteger(before.size) || before.size > MAX_LIVE_FILE_BYTES) {
        throw new AexConfigError("live upload exceeds the five GiB limit");
      }
      const digest = await hashHandle(handle, before.size);
      const replay = options.idempotencyKey ?? `live-file-${randomUUID()}`;
      const upload = await this.#executor.execute<LiveFileUpload>(
        "session_files_live_upload_create",
        { sessionId: options.sessionId },
        {
          body: {
            path: options.path,
            sizeBytes: String(before.size),
            sha256: digest,
            mode: options.mode,
            ifGenerationId: options.ifGenerationId,
          },
          idempotencyKey: replayKey("upload-open", replay),
        },
      );
      validateUpload(upload, options, before.size, digest);
      const receipts = new Map(upload.parts.map((part) => [part.partNumber, part]));
      await mapBounded(partNumbers(upload.partCount), concurrency, async (partNumber) => {
        const offset = (partNumber - 1) * LIVE_FILE_PART_BYTES;
        const length = Math.min(LIVE_FILE_PART_BYTES, before.size - offset);
        const bytes = await readExact(handle, offset, length);
        const sha256 = digestBytes(bytes);
        const existing = receipts.get(partNumber);
        if (existing) {
          if (decimal(existing.offset) !== offset || existing.sizeBytes !== length
            || existing.sha256 !== sha256) {
            throw new AexConfigError(`live upload part ${partNumber} conflicts with its receipt`);
          }
          return;
        }
        const current = await this.#executor.execute<LiveFileUpload>(
          "session_files_live_upload_part_put",
          { sessionId: options.sessionId, fileUploadId: upload.id, partNumber: String(partNumber) },
          { query: { sha256 }, body: bytes },
        );
        validateUpload(current, options, before.size, digest, upload.id, upload.generationId);
      });
      const after = await handle.stat();
      if (after.size !== before.size || after.mtimeMs !== before.mtimeMs) {
        throw new AexConfigError("live upload source changed during transfer");
      }
      const complete = await this.#executor.execute<LiveFileUpload>(
        "session_files_live_upload_complete",
        { sessionId: options.sessionId, fileUploadId: upload.id },
        { idempotencyKey: replayKey("upload-complete", replay) },
      );
      validateUpload(complete, options, before.size, digest, upload.id, upload.generationId);
      if (complete.state !== "complete" || complete.parts.length !== complete.partCount) {
        throw new AexConfigError("live upload completion returned incomplete state");
      }
      return complete;
    } finally {
      await handle.close();
    }
  }

  async downloadFile(options: DownloadLiveFileOptions): Promise<LiveFileDownload> {
    const concurrency = boundedConcurrency(options.concurrency);
    const replay = options.idempotencyKey ?? `live-file-${randomUUID()}`;
    const descriptor = await this.#executor.execute<LiveFileDownload>(
      "session_files_live_download_create",
      { sessionId: options.sessionId },
      {
        body: { path: options.path, ifGenerationId: options.ifGenerationId },
        idempotencyKey: replayKey("download-open", replay),
      },
    );
    const size = validateDownload(descriptor, options);
    const temporary = join(
      dirname(options.destinationPath),
      `.${basename(options.destinationPath)}.aex-${randomUUID()}.part`,
    );
    let handle: FileHandle | undefined;
    let published = false;
    try {
      handle = await open(temporary, "wx+");
      await handle.truncate(size);
      await mapBounded(descriptor.parts, concurrency, async (part) => {
        const bytes = await this.#executor.execute<Uint8Array>(
          "session_files_live_download_part_get",
          {
            sessionId: options.sessionId,
            fileDownloadId: descriptor.id,
            partNumber: String(part.partNumber),
          },
          { query: { version: descriptor.version } },
        );
        if (!(bytes instanceof Uint8Array) || bytes.byteLength !== part.sizeBytes
          || digestBytes(bytes) !== part.sha256) {
          throw new AexConfigError(`live download part ${part.partNumber} failed integrity`);
        }
        await writeExact(handle!, bytes, decimal(part.offset));
      });
      await handle.sync();
      const actual = await hashHandle(handle, size);
      if (actual !== descriptor.sha256) {
        throw new AexConfigError("live download whole-file digest mismatch");
      }
      const verified = await this.#executor.execute<LiveFileDownload>(
        "session_files_live_download_complete",
        { sessionId: options.sessionId, fileDownloadId: descriptor.id },
        {
          body: { version: descriptor.version, sha256: actual },
          idempotencyKey: replayKey("download-complete", replay),
        },
      );
      validateVerifiedDownload(verified, descriptor);
      await handle.close();
      handle = undefined;
      if (options.replace) {
        await rename(temporary, options.destinationPath);
      } else {
        await link(temporary, options.destinationPath);
        await unlink(temporary);
      }
      published = true;
      return verified;
    } finally {
      if (handle) await handle.close().catch(() => undefined);
      if (!published) await unlink(temporary).catch(() => undefined);
      if (!published) {
        await this.#executor.execute(
          "session_files_live_download_delete",
          { sessionId: options.sessionId, fileDownloadId: descriptor.id },
        ).catch(() => undefined);
      }
    }
  }
}

function boundedConcurrency(value = DEFAULT_LIVE_FILE_CONCURRENCY): number {
  if (!Number.isInteger(value) || value < 1 || value > MAX_LIVE_FILE_CONCURRENCY) {
    throw new AexConfigError(`live-file concurrency must be 1..${MAX_LIVE_FILE_CONCURRENCY}`);
  }
  return value;
}

function replayKey(phase: string, seed: string): string {
  return createHash("sha256").update(`aex.live-file.${phase}.v1\0${seed}`).digest("hex");
}

function digestBytes(bytes: Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}

async function hashHandle(handle: FileHandle, size: number): Promise<string> {
  const digest = createHash("sha256");
  const buffer = Buffer.allocUnsafe(Math.min(1_048_576, Math.max(size, 1)));
  let offset = 0;
  while (offset < size) {
    const length = Math.min(buffer.byteLength, size - offset);
    const { bytesRead } = await handle.read(buffer, 0, length, offset);
    if (bytesRead !== length) throw new AexConfigError("file changed or ended during transfer");
    digest.update(buffer.subarray(0, bytesRead));
    offset += bytesRead;
  }
  return digest.digest("hex");
}

async function readExact(handle: FileHandle, offset: number, length: number): Promise<Uint8Array> {
  const buffer = Buffer.allocUnsafe(length);
  let read = 0;
  while (read < length) {
    const result = await handle.read(buffer, read, length - read, offset + read);
    if (result.bytesRead === 0) throw new AexConfigError("file changed or ended during transfer");
    read += result.bytesRead;
  }
  return buffer;
}

async function writeExact(handle: FileHandle, bytes: Uint8Array, offset: number): Promise<void> {
  let written = 0;
  while (written < bytes.byteLength) {
    const result = await handle.write(bytes, written, bytes.byteLength - written, offset + written);
    if (result.bytesWritten === 0) throw new AexConfigError("temporary download write made no progress");
    written += result.bytesWritten;
  }
}

function partNumbers(count: number): readonly number[] {
  return Array.from({ length: count }, (_, index) => index + 1);
}

async function mapBounded<T>(
  values: readonly T[],
  concurrency: number,
  task: (value: T) => Promise<void>,
): Promise<void> {
  let next = 0;
  await Promise.all(Array.from({ length: Math.min(concurrency, values.length) }, async () => {
    while (true) {
      const index = next++;
      if (index >= values.length) return;
      await task(values[index]!);
    }
  }));
}

function decimal(value: string): number {
  if (!/^(0|[1-9][0-9]*)$/.test(value)) throw new AexConfigError("invalid canonical decimal");
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed)) throw new AexConfigError("decimal exceeds SDK precision");
  return parsed;
}

function validateUpload(
  upload: LiveFileUpload,
  options: UploadLiveFileOptions,
  size: number,
  sha256: string,
  id?: string,
  generation?: string,
): void {
  if (upload.sessionId !== options.sessionId || upload.path !== options.path
    || decimal(upload.sizeBytes) !== size || upload.sha256 !== sha256
    || upload.partSizeBytes !== LIVE_FILE_PART_BYTES
    || upload.partCount !== Math.ceil(size / LIVE_FILE_PART_BYTES)
    || (id !== undefined && upload.id !== id)
    || (generation !== undefined && upload.generationId !== generation)) {
    throw new AexConfigError("live upload response does not match the requested transfer");
  }
  validateParts(upload.parts, size, upload.partCount);
}

function validateDownload(download: LiveFileDownload, options: DownloadLiveFileOptions): number {
  const size = decimal(download.sizeBytes);
  if (download.sessionId !== options.sessionId || download.path !== options.path
    || download.state !== "open" || size > MAX_LIVE_FILE_BYTES
    || download.partSizeBytes !== LIVE_FILE_PART_BYTES
    || download.partCount !== Math.ceil(size / LIVE_FILE_PART_BYTES)) {
    throw new AexConfigError("invalid live download descriptor");
  }
  validateParts(download.parts, size, download.partCount);
  if (download.parts.length !== download.partCount) {
    throw new AexConfigError("live download descriptor omits parts");
  }
  return size;
}

function validateParts(parts: readonly LiveFilePart[], size: number, count: number): void {
  let previous = 0;
  const seen = new Set<number>();
  for (const part of parts) {
    const offset = decimal(part.offset);
    const expectedSize = Math.min(LIVE_FILE_PART_BYTES, size - offset);
    if (!Number.isInteger(part.partNumber) || part.partNumber <= previous || part.partNumber > count
      || seen.has(part.partNumber) || offset !== (part.partNumber - 1) * LIVE_FILE_PART_BYTES
      || part.sizeBytes !== expectedSize || part.sizeBytes < 1
      || !/^[0-9a-f]{64}$/.test(part.sha256)) {
      throw new AexConfigError("invalid live-file part descriptor");
    }
    seen.add(part.partNumber);
    previous = part.partNumber;
  }
}

function validateVerifiedDownload(verified: LiveFileDownload, opened: LiveFileDownload): void {
  if (verified.state !== "verified" || verified.id !== opened.id
    || verified.sessionId !== opened.sessionId || verified.generationId !== opened.generationId
    || verified.path !== opened.path || verified.sizeBytes !== opened.sizeBytes
    || verified.sha256 !== opened.sha256 || verified.version !== opened.version) {
    throw new AexConfigError("live download verification changed its descriptor");
  }
}
