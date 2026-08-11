import { createHash } from "node:crypto";
import { mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, test } from "bun:test";

import {
  AexConfigError,
  type ExecuteOptions,
  type ResourceExecutor,
  type RouteId,
} from "../../src/index.js";
import {
  LIVE_FILE_PART_BYTES,
  SessionFiles,
  type LiveFileDownload,
  type LiveFilePart,
  type LiveFileUpload,
} from "../../src/node/session-files.js";

const roots: string[] = [];
const SESSION = "ses_0100000000e008000000000001";
const GENERATION = "gen_0100000000e008000000000002";

afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })));
});

class LiveFileExecutor implements ResourceExecutor {
  readonly calls: { route: RouteId; bindings: Readonly<Record<string, string>>; options: ExecuteOptions }[] = [];
  readonly bytes: Uint8Array;
  readonly parts: LiveFilePart[];
  readonly uploadReceipts: LiveFilePart[];
  corruptPart = 0;

  constructor(bytes: Uint8Array, existingUploadParts: readonly number[] = []) {
    this.bytes = bytes;
    this.parts = describeParts(bytes);
    this.uploadReceipts = this.parts.filter((part) => existingUploadParts.includes(part.partNumber));
  }

  async execute<T>(
    route: RouteId,
    bindings: Readonly<Record<string, string>> = {},
    options: ExecuteOptions = {},
  ): Promise<T> {
    this.calls.push({ route, bindings, options });
    switch (route) {
      case "session_files_live_upload_create":
      case "session_files_live_upload_get":
        return this.upload("staging") as T;
      case "session_files_live_upload_part_put": {
        const part = this.parts[Number(bindings.partNumber) - 1]!;
        const body = options.body as Uint8Array;
        expect(body.byteLength).toBe(part.sizeBytes);
        expect(hash(body)).toBe(part.sha256);
        if (!this.uploadReceipts.some((receipt) => receipt.partNumber === part.partNumber)) {
          this.uploadReceipts.push(part);
          this.uploadReceipts.sort((left, right) => left.partNumber - right.partNumber);
        }
        return this.upload("staging") as T;
      }
      case "session_files_live_upload_complete":
        return this.upload("complete") as T;
      case "session_files_live_download_create":
        return this.download("open") as T;
      case "session_files_live_download_part_get": {
        const part = this.parts[Number(bindings.partNumber) - 1]!;
        const offset = Number(part.offset);
        const result = this.bytes.slice(offset, offset + part.sizeBytes);
        if (part.partNumber === this.corruptPart) result[0] = (result[0] ?? 0) ^ 0xff;
        return result as T;
      }
      case "session_files_live_download_complete":
        return this.download("verified") as T;
      case "session_files_live_download_delete":
        return undefined as T;
      default:
        throw new Error(`unexpected ${route}`);
    }
  }

  upload(state: "staging" | "complete"): LiveFileUpload {
    return {
      id: "fup_0100000000e008000000000003",
      sessionId: SESSION,
      generationId: GENERATION,
      path: "/large.bin",
      state,
      sizeBytes: String(this.bytes.byteLength),
      sha256: hash(this.bytes),
      mode: "0644",
      partSizeBytes: LIVE_FILE_PART_BYTES,
      partCount: this.parts.length,
      parts: state === "complete" ? this.parts : this.uploadReceipts,
      expiresAt: "2999-01-01T00:00:00Z",
      workspaceAccess: { generationId: GENERATION, resumed: false },
    };
  }

  download(state: "open" | "verified"): LiveFileDownload {
    return {
      id: "fdl_0100000000e008000000000004",
      sessionId: SESSION,
      generationId: GENERATION,
      path: "/large.bin",
      state,
      sizeBytes: String(this.bytes.byteLength),
      sha256: hash(this.bytes),
      version: "1".repeat(64),
      partSizeBytes: LIVE_FILE_PART_BYTES,
      partCount: this.parts.length,
      parts: this.parts,
      expiresAt: "2999-01-01T00:00:00Z",
      workspaceAccess: { generationId: GENERATION, resumed: false },
    };
  }
}

describe("direct live session files", () => {
  test("resumes verified upload parts and completes without whole-file buffering", async () => {
    const root = await temporaryRoot();
    const source = join(root, "source.bin");
    const bytes = fixtureBytes(LIVE_FILE_PART_BYTES + 17);
    await writeFile(source, bytes);
    const executor = new LiveFileExecutor(bytes, [2]);

    const result = await new SessionFiles(executor).uploadFile({
      sessionId: SESSION,
      path: "/large.bin",
      sourcePath: source,
      concurrency: 2,
      idempotencyKey: "stable-upload",
    });

    expect(result.state).toBe("complete");
    expect(executor.calls.filter((call) => call.route === "session_files_live_upload_part_put")
      .map((call) => call.bindings.partNumber)).toEqual(["1"]);
    expect(executor.calls.at(-1)?.route).toBe("session_files_live_upload_complete");
  });

  test("downloads exact ranges to a temporary target and publishes after verification", async () => {
    const root = await temporaryRoot();
    const destination = join(root, "download.bin");
    const bytes = fixtureBytes(LIVE_FILE_PART_BYTES + 31);
    const executor = new LiveFileExecutor(bytes);

    const result = await new SessionFiles(executor).downloadFile({
      sessionId: SESSION,
      path: "/large.bin",
      destinationPath: destination,
      concurrency: 2,
      idempotencyKey: "stable-download",
    });

    expect(result.state).toBe("verified");
    expect(await readFile(destination)).toEqual(Buffer.from(bytes));
    expect((await readdir(root)).filter((name) => name.endsWith(".part"))).toEqual([]);
    expect(executor.calls.filter((call) => call.route === "session_files_live_download_part_get"))
      .toHaveLength(2);
  });

  test("never publishes a corrupt ranged download", async () => {
    const root = await temporaryRoot();
    const destination = join(root, "download.bin");
    const bytes = fixtureBytes(LIVE_FILE_PART_BYTES + 1);
    const executor = new LiveFileExecutor(bytes);
    executor.corruptPart = 2;

    await expect(new SessionFiles(executor).downloadFile({
      sessionId: SESSION,
      path: "/large.bin",
      destinationPath: destination,
    })).rejects.toBeInstanceOf(AexConfigError);

    expect(await readdir(root)).toEqual([]);
    expect(executor.calls.at(-1)?.route).toBe("session_files_live_download_delete");
  });
});

async function temporaryRoot(): Promise<string> {
  const root = await mkdtemp(join(tmpdir(), "aex-live-files-"));
  roots.push(root);
  return root;
}

function fixtureBytes(length: number): Uint8Array {
  return Uint8Array.from({ length }, (_, index) => (index * 31 + 7) & 0xff);
}

function hash(bytes: Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}

function describeParts(bytes: Uint8Array): LiveFilePart[] {
  const parts: LiveFilePart[] = [];
  for (let offset = 0, partNumber = 1; offset < bytes.byteLength;
    offset += LIVE_FILE_PART_BYTES, partNumber += 1) {
    const value = bytes.slice(offset, Math.min(bytes.byteLength, offset + LIVE_FILE_PART_BYTES));
    parts.push({
      partNumber,
      offset: String(offset),
      sizeBytes: value.byteLength,
      sha256: hash(value),
    });
  }
  return parts;
}
