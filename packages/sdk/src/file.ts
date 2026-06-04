import { readdir, readFile, stat } from "node:fs/promises";
import { join, relative } from "node:path";
import type { FileRef } from "@aexhq/contracts";
import { hashSkillBundle } from "./bundle.js";
import { zipSync } from "fflate";

/**
 * File — arbitrary bytes (single file or zipped folder) delivered to
 * the agent as a mounted runtime resource. The managed runtime receives the
 * snapshotted bytes during workspace materialization.
 *
 *   const settings = await File.fromPath("./settings.json");
 *   const dataset = await File.fromPath("./data/");
 *   await client.submitRun({ files: [settings, dataset], ... });
 *
 * `client.submitRun` materializes the bytes to the hosted asset store before
 * the run lands; the wire ref becomes `kind:"asset"`. Repeat uploads of the
 * same bytes are deduped.
 */
export class File {
  readonly #ref: FileRef | DraftFileRef;
  readonly #bytes: Uint8Array | undefined;
  #consumed = false;

  constructor(ref: FileRef | DraftFileRef, bytes?: Uint8Array) {
    this.#ref = ref;
    this.#bytes = bytes;
  }

  get ref(): FileRef | DraftFileRef {
    return this.#ref;
  }

  get isDraft(): boolean {
    return this.#ref.kind === "draft" && !this.#consumed;
  }

  get isConsumed(): boolean {
    return this.#consumed;
  }

  /**
   * Build a draft File from raw bytes. The bytes are wrapped in a
   * single-entry canonical zip so storage stays uniform across all
   * file uploads.
   */
  static async fromBytes(args: {
    readonly name: string;
    readonly bytes: Uint8Array;
    readonly mountPath?: string;
  }): Promise<File> {
    if (!args || typeof args.name !== "string" || !WORKSPACE_NAME_RE.test(args.name)) {
      throw new Error(`File.fromBytes: name must match ${WORKSPACE_NAME_RE.source}`);
    }
    if (!(args.bytes instanceof Uint8Array) || args.bytes.byteLength === 0) {
      throw new Error("File.fromBytes: bytes must be a non-empty Uint8Array");
    }
    const zip = zipSync(
      { [args.name]: [args.bytes, { mtime: ZIP_EPOCH }] },
      { level: 6 }
    );
    const contentHash = await hashSkillBundle(zip);
    const ref: DraftFileRef = {
      kind: "draft",
      name: args.name,
      contentHash,
      ...(args.mountPath ? { mountPath: args.mountPath } : {})
    };
    return new File(ref, zip);
  }

  /**
   * Read a local file or directory and build a draft File. Directories
   * walk recursively into a canonical zip (sorted paths, deterministic
   * mtime). Name is inferred from the basename when not supplied.
   */
  static async fromPath(
    path: string,
    args?: { readonly name?: string; readonly mountPath?: string }
  ): Promise<File> {
    const stats = await stat(path);
    const inferredName = args?.name ?? inferNameFromPath(path);
    if (!WORKSPACE_NAME_RE.test(inferredName)) {
      throw new Error(
        `File.fromPath: inferred name ${JSON.stringify(inferredName)} does not match ${WORKSPACE_NAME_RE.source}; ` +
          `pass an explicit name via args.name`
      );
    }
    let zip: Uint8Array;
    if (stats.isDirectory()) {
      zip = await buildDirZip(path);
    } else {
      const bytes = await readFile(path);
      const filename = path.replace(/\\/g, "/").split("/").at(-1) ?? inferredName;
      zip = zipSync({ [filename]: [bytes, { mtime: ZIP_EPOCH }] }, { level: 6 });
    }
    const contentHash = await hashSkillBundle(zip);
    const ref: DraftFileRef = {
      kind: "draft",
      name: inferredName,
      contentHash,
      ...(args?.mountPath ? { mountPath: args.mountPath } : {})
    };
    return new File(ref, zip);
  }

  /**
   * Internal: yield the draft's zipped bytes + metadata so
   * `client.submitRun` can upload it as an asset.
   */
  _takeDraftBundle(): {
    name: string;
    contentHash: string;
    bytes: Uint8Array;
    mountPath?: string;
  } | undefined {
    if (this.#consumed) {
      throw new Error(
        "File: cannot reuse a consumed File in submitRun. Build a fresh File via " +
          "File.fromPath(...) / File.fromBytes(...) per submitRun call."
      );
    }
    if (this.#ref.kind !== "draft" || !this.#bytes) {
      return undefined;
    }
    this.#consumed = true;
    return {
      name: this.#ref.name,
      contentHash: this.#ref.contentHash,
      bytes: this.#bytes,
      ...(this.#ref.mountPath ? { mountPath: this.#ref.mountPath } : {})
    };
  }

  toJSON(): FileRef {
    if (this.#ref.kind === "draft") {
      throw new Error(
        "File: draft Files cannot be JSON-serialised — they only become wire refs when " +
        "client.submitRun uploads the bytes as an asset."
      );
    }
    return this.#ref;
  }
}

export interface DraftFileRef {
  readonly kind: "draft";
  readonly name: string;
  readonly contentHash: string;
  readonly mountPath?: string;
}

const ZIP_EPOCH = new Date(Date.UTC(1980, 0, 1));
const WORKSPACE_NAME_RE = /^[a-z0-9][a-z0-9-]{0,62}[a-z0-9]$/;

function inferNameFromPath(path: string): string {
  const normalised = path.replace(/\\/g, "/").replace(/\/+$/, "");
  const base = normalised.split("/").at(-1) ?? "";
  const stem = base.replace(/\.[^.]+$/, "").toLowerCase();
  const slug = stem.replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
  if (slug.length >= 2 && WORKSPACE_NAME_RE.test(slug)) return slug;
  return `file-${Date.now().toString(36)}`;
}

async function buildDirZip(dirPath: string): Promise<Uint8Array> {
  const files: Array<{ rel: string; bytes: Uint8Array }> = [];
  await walkDir(dirPath, dirPath, files);
  if (files.length === 0) {
    throw new Error(`File.fromPath: directory ${JSON.stringify(dirPath)} is empty (no regular files)`);
  }
  files.sort((a, b) => (a.rel < b.rel ? -1 : a.rel > b.rel ? 1 : 0));
  const zippable: Record<string, [Uint8Array, { mtime: Date }]> = {};
  for (const { rel, bytes } of files) {
    zippable[rel] = [bytes, { mtime: ZIP_EPOCH }];
  }
  return zipSync(zippable, { level: 6 });
}

async function walkDir(
  base: string,
  current: string,
  result: Array<{ rel: string; bytes: Uint8Array }>
): Promise<void> {
  const entries = await readdir(current, { withFileTypes: true });
  for (const entry of entries) {
    const fullPath = join(current, entry.name);
    if (entry.isDirectory()) {
      await walkDir(base, fullPath, result);
    } else if (entry.isFile()) {
      const bytes = await readFile(fullPath);
      const rel = relative(base, fullPath).replace(/\\/g, "/");
      result.push({ rel, bytes: new Uint8Array(bytes) });
    }
  }
}
