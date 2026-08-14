import type { ResourceExecutor } from "../generated/resources.js";

/** Inline, URL and direct-upload inputs accepted by the latest-only file API. */
export type WorkspaceFileInput =
  | { readonly type: "bytes"; readonly bytes: Uint8Array }
  | { readonly type: "text"; readonly text: string }
  | { readonly type: "url"; readonly url: string };

/** One current workspace-file response. No public version identity exists. */
export interface CurrentWorkspaceFile {
  readonly name: string;
  readonly state: "pending" | "ready" | "failed";
  readonly value?: {
    readonly content: { readonly sha256: string; readonly sizeBytes: string };
    readonly mediaType: string;
    readonly mode: "0644" | "0755";
  };
}

interface UploadAdmission {
  readonly upload: {
    readonly id: string;
    readonly partCount: number;
    readonly partSizeBytes: string;
  };
  readonly grants: ReadonlyArray<{
    readonly partNumber: number;
    readonly url: string;
    readonly headers: ReadonlyArray<{ readonly name: string; readonly value: string }>;
  }>;
}

interface DownloadGrant {
  readonly url: string;
  readonly sha256: string;
  readonly sizeBytes: string;
}

export interface WorkspaceFilePutOptions {
  readonly mediaType?: string;
  readonly mode?: "0644" | "0755";
  readonly idempotencyKey?: string;
}

const UPLOAD_MIN_PART_BYTES = 5 * 1024 * 1024;
const UPLOAD_MAX_PART_BYTES = 5 * 1024 * 1024 * 1024;
const UPLOAD_MAX_PARTS = 1_000;

/**
 * Convenience surface over the seven latest-only file/upload operations.
 *
 * Direct object-store transfers replay only the exact signed headers returned
 * by Aex. The guest is never given AWS credentials; `storage.persist` uses the
 * same private grant shape from inside Tool Mux.
 */
export class WorkspaceFiles {
  readonly #executor: ResourceExecutor;
  readonly #fetch: typeof globalThis.fetch;

  constructor(executor: ResourceExecutor, fetchLike = globalThis.fetch) {
    this.#executor = executor;
    this.#fetch = fetchLike.bind(globalThis);
  }

  async put(
    name: string,
    input: WorkspaceFileInput,
    options: WorkspaceFilePutOptions = {},
  ): Promise<CurrentWorkspaceFile> {
    const mediaType = options.mediaType ?? (input.type === "text" ? "text/plain" : "application/octet-stream");
    const mode = options.mode ?? "0644";
    const content = input.type === "url"
      ? { type: "url", url: input.url }
      : await inlineInput(input.type === "text" ? new TextEncoder().encode(input.text) : input.bytes);
    return this.#executor.execute<CurrentWorkspaceFile>(
      "registry_files_put",
      { name },
      {
        idempotencyKey: options.idempotencyKey ?? globalThis.crypto.randomUUID(),
        body: { content, mediaType, mode },
      },
    );
  }

  /** Uploads arbitrary bytes and publishes them to the admitted logical name. */
  async upload(
    name: string,
    bytes: Uint8Array,
    options: WorkspaceFilePutOptions = {},
  ): Promise<CurrentWorkspaceFile> {
    const plannedParts = planUploadParts(bytes.byteLength);
    const wholeHash = await sha256(bytes);
    const declaredParts = await Promise.all(plannedParts.map(async ({ partNumber, start, end }) => ({
      partNumber,
      sha256: await sha256(bytes.subarray(start, end)),
      sizeBytes: String(end - start),
    })));
    const admission = await this.#executor.execute<UploadAdmission>(
      "upload_create",
      {},
      {
        idempotencyKey: options.idempotencyKey ?? globalThis.crypto.randomUUID(),
        body: {
          name,
          sizeBytes: String(bytes.byteLength),
          sha256: wholeHash,
          contentType: options.mediaType ?? "application/octet-stream",
          parts: declaredParts,
        },
      },
    );
    if (
      admission.upload.partCount !== declaredParts.length
      || admission.grants.length !== admission.upload.partCount
    ) {
      throw new Error("upload admission omitted one or more part grants");
    }
    const partSize = Number(admission.upload.partSizeBytes);
    if (
      !Number.isSafeInteger(partSize)
      || partSize <= 0
      || partSize !== plannedParts[0]?.end
    ) {
      throw new Error("upload admission returned an invalid part size");
    }
    const parts: Array<{ partNumber: number; etag: string }> = [];
    for (const [index, grant] of admission.grants.entries()) {
      if (grant.partNumber !== index + 1) {
        throw new Error("upload admission returned non-contiguous part grants");
      }
      const start = index * partSize;
      const end = Math.min(start + partSize, bytes.byteLength);
      const headers = new Headers(grant.headers.map(({ name: header, value }) => [header, value]));
      const response = await this.#fetch(grant.url, {
        method: "PUT",
        headers,
        body: toBody(bytes.slice(start, end)),
        redirect: "error",
      });
      if (!response.ok) throw new Error(`object-store upload failed with HTTP ${response.status}`);
      const etag = response.headers.get("etag");
      if (etag === null) throw new Error("object-store upload response omitted ETag");
      parts.push({ partNumber: grant.partNumber, etag });
    }
    return this.#executor.execute<CurrentWorkspaceFile>(
      "upload_complete",
      { uploadId: admission.upload.id },
      {
        idempotencyKey: globalThis.crypto.randomUUID(),
        body: { parts },
      },
    );
  }

  async get(name: string): Promise<CurrentWorkspaceFile> {
    return this.#executor.execute<CurrentWorkspaceFile>("registry_files_get", { name });
  }

  async list(query: { readonly cursor?: string; readonly limit?: number } = {}): Promise<{
    readonly items: ReadonlyArray<CurrentWorkspaceFile>;
    readonly nextCursor?: string;
  }> {
    return this.#executor.execute("registry_files_list", {}, {
      query: {
        ...(query.cursor === undefined ? {} : { cursor: query.cursor }),
        ...(query.limit === undefined ? {} : { limit: String(query.limit) }),
      },
    });
  }

  async delete(name: string): Promise<void> {
    await this.#executor.execute("registry_files_delete", { name });
  }

  /** Downloads and verifies the exact current ready bytes. */
  async download(name: string): Promise<Uint8Array> {
    const grant = await this.#executor.execute<DownloadGrant>(
      "registry_files_download_create",
      { name },
      { idempotencyKey: globalThis.crypto.randomUUID(), body: {} },
    );
    const response = await this.#fetch(grant.url, { redirect: "error" });
    if (!response.ok) throw new Error(`workspace-file download failed with HTTP ${response.status}`);
    const bytes = new Uint8Array(await response.arrayBuffer());
    if (String(bytes.byteLength) !== grant.sizeBytes || await sha256(bytes) !== grant.sha256) {
      throw new Error("workspace-file download failed exact length/hash verification");
    }
    return bytes;
  }
}

function planUploadParts(sizeBytes: number): ReadonlyArray<{
  readonly partNumber: number;
  readonly start: number;
  readonly end: number;
}> {
  if (!Number.isSafeInteger(sizeBytes) || sizeBytes <= 0) {
    throw new TypeError("direct upload requires a non-empty safe-integer byte length");
  }
  let partSize = Math.min(sizeBytes, UPLOAD_MIN_PART_BYTES);
  while (Math.ceil(sizeBytes / partSize) > UPLOAD_MAX_PARTS) {
    partSize = Math.min(partSize * 2, UPLOAD_MAX_PART_BYTES);
    if (partSize === UPLOAD_MAX_PART_BYTES && Math.ceil(sizeBytes / partSize) > UPLOAD_MAX_PARTS) {
      throw new RangeError("direct upload exceeds the 1000-part admission limit");
    }
  }
  const parts = [];
  for (let start = 0, partNumber = 1; start < sizeBytes; start += partSize, partNumber += 1) {
    parts.push({ partNumber, start, end: Math.min(start + partSize, sizeBytes) });
  }
  return parts;
}

async function inlineInput(bytes: Uint8Array): Promise<{
  readonly type: "inline";
  readonly encoding: "base64";
  readonly data: string;
  readonly sha256: string;
}> {
  return { type: "inline", encoding: "base64", data: base64(bytes), sha256: await sha256(bytes) };
}

function base64(bytes: Uint8Array): string {
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
  }
  return btoa(binary);
}

async function sha256(bytes: Uint8Array): Promise<string> {
  const digest = new Uint8Array(await globalThis.crypto.subtle.digest("SHA-256", toBufferSource(bytes)));
  return `sha256:${[...digest].map((byte) => byte.toString(16).padStart(2, "0")).join("")}`;
}

function toBufferSource(bytes: Uint8Array): Uint8Array<ArrayBuffer> {
  return bytes.buffer instanceof ArrayBuffer
    ? new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength)
    : Uint8Array.from(bytes);
}

function toBody(bytes: Uint8Array): Uint8Array<ArrayBuffer> {
  return toBufferSource(bytes);
}
