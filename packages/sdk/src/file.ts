import { createReadStream } from "node:fs";
import { readFile, stat } from "node:fs/promises";
import {
  ASSET_ARCHIVE_LIMITS,
  DEFAULT_FILE_MOUNT_PATH,
  RESERVED_META_ENTRY,
  assertValidMountPath,
  bundleManifestIsEmpty,
  serializeBundleManifest
} from "@aexhq/contracts";
import { bundleSingleFile, hashSkillBundle, type BundleMeta } from "@aexhq/contracts/internal";
import {
  frameCanonicalZipSync,
  streamBundleZip,
  type ByteSink,
  type ZipEntrySource
} from "./canonical-zip.js";
import { walkDirectory, type IgnoreOptions } from "./node-walk.js";
import { zipSync } from "fflate";
import {
  assertArchiveCompressedSize,
  assertArchiveEntryCount,
  assertArchiveExpandedSize
} from "./archive-limits.js";

/**
 * File — arbitrary bytes (single file or zipped folder) delivered to
 * the agent as a mounted runtime resource. The managed runtime UNZIPS the
 * snapshotted bytes into the `mountPath` DIRECTORY during workspace
 * materialization, preserving the real filename + extension.
 *
 *   const settings = await File.fromPath("./settings.json");
 *   const dataset = await File.fromPath("./data/");
 *   const input = await client.workspace.files.publish(settings);
 *   await client.start({ assets: { files: [input] }, message: "..." });
 *
 * `mountPath` is the absolute container directory the file unzips into; it
 * defaults to `/workspace` (the agent's default working directory), so a file
 * handed with no `mountPath` lands directly in the agent's cwd — a single file
 * `subtitles.srt` becomes `/workspace/subtitles.srt`, a folder lands its entries
 * under `/workspace/`. The resolved path is surfaced back on the Session record.
 *
 * FIDELITY: a directory walk captures executable bits and symlinks into a
 * `.aexmeta.json` sidecar (emitted only when such metadata exists, so a
 * pure-content bundle stays byte-identical → dedup continuity), and honors
 * `.aexignore` (gitignore-compatible) plus always-on defaults (`.git/`,
 * `node_modules/`). SCALE: a large input (total raw size over the streaming
 * threshold, `File.fromPath` only) is uploaded via a streaming two-pass
 * multipart flow that holds only one entry + one part in memory within the
 * runtime asset archive envelope.
 *
 * Publish drafts through `aex.workspace.files.publish(file)`, then pin the
 * returned immutable ref in `assets.files`.
 */
export class File {
  readonly #ref: DraftFileRef;
  readonly #bytes: Uint8Array | undefined;
  /** Large-input streaming driver — re-runs the deterministic canonical-zip framer per pass. */
  readonly #drive: ZipStreamDriver | undefined;
  private constructor(ref: DraftFileRef, bytes?: Uint8Array, drive?: ZipStreamDriver) {
    this.#ref = ref;
    this.#bytes = bytes;
    this.#drive = drive;
  }

  get ref(): DraftFileRef {
    return this.#ref;
  }

  get isDraft(): boolean {
    return true;
  }

  /**
   * Build a draft File from raw bytes. `name` is the REAL filename (with its
   * extension, e.g. `"subtitles.srt"`) — it is preserved as the single zip
   * entry so the agent finds it at `<mountPath>/<name>` after unzip. The bytes
   * are wrapped in a single-entry canonical zip so storage stays uniform across
   * all file uploads. `mountPath` is the absolute container directory the file
   * unzips into; it defaults to `/workspace`.
   */
  static async fromBytes(args: {
    readonly name: string;
    readonly bytes: Uint8Array;
    readonly mountPath?: string;
  }): Promise<File> {
    if (!args || typeof args.name !== "string") {
      throw new Error("File.fromBytes: name must be a string");
    }
    const filename = sanitiseFilename(args.name);
    if (filename === undefined) {
      throw new Error(
        `File.fromBytes: name ${JSON.stringify(args.name)} is not a valid filename ` +
          "(no '/', '\\\\', NUL, or path traversal; 1..255 chars)"
      );
    }
    if (!(args.bytes instanceof Uint8Array) || args.bytes.byteLength === 0) {
      throw new Error("File.fromBytes: bytes must be a non-empty Uint8Array");
    }
    const mountPath = resolveMountPath(args.mountPath, "File.fromBytes");
    const zip = bundleSingleFile(filename, args.bytes, "File.fromBytes");
    const contentHash = await hashSkillBundle(zip);
    const ref: DraftFileRef = {
      kind: "draft",
      name: slugFromFilename(filename),
      contentHash,
      mountPath
    };
    return new File(ref, zip);
  }

  /**
   * Read a local file or directory and build a draft File. A single file
   * preserves its real basename (with extension) as the sole zip entry, so the
   * agent finds it at `<mountPath>/<basename>` after unzip. Directories walk
   * recursively into a canonical zip (sorted relative paths, deterministic
   * mtime), landing each entry under `<mountPath>/`. Executable bits + symlinks
   * are captured into a `.aexmeta.json` sidecar; `.aexignore` (+ defaults) prune
   * the walk. `mountPath` defaults to `/workspace`. The optional `name` is only
   * the storage slug (dedup label); it never affects the on-disk filename.
   */
  static async fromPath(
    path: string,
    args?: { readonly name?: string; readonly mountPath?: string; readonly ignore?: IgnoreOptions }
  ): Promise<File> {
    const stats = await stat(path);
    const mountPath = resolveMountPath(args?.mountPath, "File.fromPath");
    // `name` is only the storage slug (dedup label), never the on-disk filename.
    const slug = args?.name !== undefined ? slugFromFilename(args.name) : inferNameFromPath(path);
    return stats.isDirectory()
      ? File.#fromDirectory(path, slug, mountPath, args?.ignore)
      : File.#fromSingleFile(path, slug, mountPath, stats.size, (stats.mode & 0o111) !== 0);
  }

  /** Directory branch: walk + fidelity sidecar + small/streaming path selection. */
  static async #fromDirectory(
    path: string,
    slug: string,
    mountPath: string,
    ignore: IgnoreOptions | undefined
  ): Promise<File> {
    const walk = await walkDirectory(path, ignore);
    if (walk.entries.length === 0 && walk.symlinks.length === 0) {
      throw new Error(`File.fromPath: directory ${JSON.stringify(path)} is empty (no regular files or symlinks)`);
    }
    const meta: BundleMeta = { exec: walk.exec, symlinks: walk.symlinks };
    const sidecar = bundleManifestIsEmpty(meta)
      ? undefined
      : serializeBundleManifest({ v: 1, exec: walk.exec, symlinks: walk.symlinks });
    assertArchiveExpandedSize(walk.totalSize + (sidecar?.byteLength ?? 0), "File.fromPath");
    assertArchiveEntryCount(
      walk.entries.length + walk.symlinks.length,
      "File.fromPath"
    );

    if (walk.totalSize <= SMALL_UPLOAD_THRESHOLD_BYTES) {
      // Small: read all bytes and build the canonical zip in memory (single PUT).
      const ordered: Array<[string, Uint8Array]> = [];
      for (const entry of walk.entries) {
        ordered.push([entry.rel, new Uint8Array(await readFile(entry.absPath))]);
      }
      if (sidecar) ordered.push([RESERVED_META_ENTRY, sidecar]);
      const zip = frameCanonicalZipSync(ordered);
      assertArchiveCompressedSize(zip.byteLength, "File.fromPath");
      const contentHash = await hashSkillBundle(zip);
      return new File({ kind: "draft", name: slug, contentHash, mountPath }, zip);
    }

    // Large: stream the canonical zip (never materialize the whole bundle).
    const sources = buildEntrySources(walk.entries, sidecar);
    const drive: ZipStreamDriver = (sink) => streamBundleZip(sources, sink);
    return new File({ kind: "draft", name: slug, mountPath }, undefined, drive);
  }

  /** Single-file branch: preserve the basename; capture the +x bit; small/streaming by size. */
  static async #fromSingleFile(
    path: string,
    slug: string,
    mountPath: string,
    size: number,
    isExecutable: boolean
  ): Promise<File> {
    const basename = path.replace(/\\/g, "/").replace(/\/+$/, "").split("/").at(-1) ?? "";
    const filename = sanitiseFilename(basename) ?? `${slug}`;
    const sidecar = isExecutable
      ? serializeBundleManifest({ v: 1, exec: [filename], symlinks: [] })
      : undefined;
    assertArchiveExpandedSize(size + (sidecar?.byteLength ?? 0), "File.fromPath");

    if (size <= SMALL_UPLOAD_THRESHOLD_BYTES) {
      const bytes = new Uint8Array(await readFile(path));
      const ordered: Array<[string, Uint8Array]> = [[filename, bytes]];
      if (sidecar) ordered.push([RESERVED_META_ENTRY, sidecar]);
      const zip = frameCanonicalZipSync(ordered);
      assertArchiveCompressedSize(zip.byteLength, "File.fromPath");
      const contentHash = await hashSkillBundle(zip);
      return new File({ kind: "draft", name: slug, contentHash, mountPath }, zip);
    }

    const sources: ZipEntrySource[] = [
      {
        name: filename,
        size,
        read: async () => new Uint8Array(await readFile(path)),
        openStream: () => fileByteStream(path)
      }
    ];
    if (sidecar) sources.push({ name: RESERVED_META_ENTRY, size: sidecar.length, read: () => sidecar });
    const drive: ZipStreamDriver = (sink) => streamBundleZip(sources, sink);
    return new File({ kind: "draft", name: slug, mountPath }, undefined, drive);
  }

  /**
   * Internal: yield the draft's zipped bytes and metadata to the workspace publisher.
   * Returns undefined for a streaming (large) draft — use {@link _takeDraftStream}.
   */
  _takeDraftBundle(): {
    name: string;
    contentHash: string;
    bytes: Uint8Array;
    mountPath: string;
  } | undefined {
    if (this.#ref.kind !== "draft" || !this.#bytes || this.#ref.contentHash === undefined) {
      return undefined;
    }
    return {
      name: this.#ref.name,
      contentHash: this.#ref.contentHash,
      bytes: this.#bytes,
      mountPath: this.#ref.mountPath
    };
  }

  /**
   * Internal: yield the draft's canonical-zip stream driver so `client.start` can
   * upload it via the streaming multipart flow. Returns undefined for the small
   * (in-memory) draft — use {@link _takeDraftBundle}.
   */
  _takeDraftStream(): { name: string; mountPath: string; drive: ZipStreamDriver } | undefined {
    if (this.#ref.kind !== "draft" || !this.#drive) return undefined;
    return { name: this.#ref.name, mountPath: this.#ref.mountPath, drive: this.#drive };
  }

  toJSON(): never {
    throw new Error("File drafts cannot be submitted directly; publish with aex.workspace.files.publish(...)");
  }
}

export interface DraftFileRef {
  readonly kind: "draft";
  readonly name: string;
  /** sha256 of the canonical zip (small path only); undefined for a streaming draft (hashed at upload). */
  readonly contentHash?: string;
  /** Absolute container directory the file unzips into (always set; defaults to /workspace). */
  readonly mountPath: string;
}

/** A retryable driver that frames the canonical zip into `sink` (one call per upload pass). */
export type ZipStreamDriver = (sink: ByteSink) => Promise<void>;

const ZIP_EPOCH = new Date(Date.UTC(1980, 0, 1));
const WORKSPACE_NAME_RE = /^[a-z0-9][a-z0-9-]{0,62}[a-z0-9]$/;

/**
 * Total raw input size (via `stat`, before reading bytes) above which
 * `File.fromPath` streams the upload instead of building the whole zip in
 * memory. A pure performance/simplicity switch, NOT a correctness boundary — the
 * streaming framer is byte-identical to the in-memory `zipSync` for Canonical A,
 * so a bundle at the threshold dedups either way.
 */
const SMALL_UPLOAD_THRESHOLD_BYTES = ASSET_ARCHIVE_LIMITS.maxCompressedBytes;

/** Build lazy zip entry sources from walked descriptors, appending the sidecar LAST. */
function buildEntrySources(
  entries: ReadonlyArray<{ readonly rel: string; readonly absPath: string; readonly size: number }>,
  sidecar: Uint8Array | undefined
): ZipEntrySource[] {
  const sources: ZipEntrySource[] = entries.map((entry) => ({
    name: entry.rel,
    size: entry.size,
    read: async () => new Uint8Array(await readFile(entry.absPath)),
    openStream: () => fileByteStream(entry.absPath)
  }));
  if (sidecar) {
    sources.push({ name: RESERVED_META_ENTRY, size: sidecar.length, read: () => sidecar });
  }
  return sources;
}

/** Async-iterate a file's bytes in native stream chunks (for Canonical-B giant entries). */
async function* fileByteStream(absPath: string): AsyncIterable<Uint8Array> {
  const stream = createReadStream(absPath);
  for await (const chunk of stream as AsyncIterable<Buffer>) {
    yield chunk;
  }
}

/**
 * Resolve + validate a caller-supplied `mountPath`, defaulting to `/workspace`.
 * Shares {@link assertValidMountPath} with the contracts parser so the SDK and
 * the BFF reject the same malformed paths.
 */
function resolveMountPath(mountPath: string | undefined, fn: string): string {
  if (mountPath === undefined) return DEFAULT_FILE_MOUNT_PATH;
  if (typeof mountPath !== "string") {
    throw new Error(`${fn}: mountPath must be a string`);
  }
  assertValidMountPath(mountPath, `${fn}: mountPath`);
  return mountPath;
}

/**
 * Validate a real on-disk filename (a single path SEGMENT — no separators,
 * traversal, NUL, or control chars; 1..255 bytes). Returns the trimmed name, or
 * `undefined` when it cannot be a filename. Used to preserve the real filename +
 * extension inside the single-entry zip.
 */
function sanitiseFilename(name: string): string | undefined {
  const trimmed = name.trim();
  if (trimmed.length === 0 || trimmed.length > 255) return undefined;
  if (trimmed === "." || trimmed === "..") return undefined;
  if (/[/\\]/.test(trimmed)) return undefined;
  // Reject NUL + C0/C1/DEL control chars (filename must stay a printable segment).
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f]/.test(trimmed)) return undefined;
  return trimmed;
}

function slugFromFilename(filename: string): string {
  const stem = filename.replace(/\.[^.]+$/, "").toLowerCase();
  const slug = stem.replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
  if (slug.length >= 2 && WORKSPACE_NAME_RE.test(slug)) return slug;
  if (slug.length === 1) return `f-${slug}`;
  return `file-${Date.now().toString(36)}`;
}

function inferNameFromPath(path: string): string {
  const normalised = path.replace(/\\/g, "/").replace(/\/+$/, "");
  const base = normalised.split("/").at(-1) ?? "";
  return slugFromFilename(base);
}
