import { zipSync, type Zippable } from "fflate";
import {
  MAX_SYMLINK_TARGET_LENGTH,
  RESERVED_META_ENTRY,
  SKILL_BUNDLE_LIMITS,
  bundleManifestIsEmpty,
  parseBundleManifest,
  serializeBundleManifest,
  validateSkillBundleEntry,
  type BundleSymlink,
  type ToolInputSchema
} from "@aexhq/contracts";
import {
  assertArchiveCompressedSize,
  assertArchiveEntryCount,
  assertArchiveExpandedSize
} from "./archive-limits.js";

/**
 * In-memory skill bundle: a flat path -> bytes map and the
 * deterministically-zipped representation.
 *
 * The SDK runs only the cheap, safety-critical checks here
 * (`validateSkillBundleEntry`: no `..`, no absolute paths, no Windows
 * backslashes, depth/length limits). The BFF re-canonicalises and
 * recomputes the canonical hash on receipt — SDK-side hashing is NOT
 * part of any contract, so we don't expose one for workspace uploads.
 *
 * For workspace skills the SDK computes an advisory `sha256` of the
 * canonicalised zip via `hashSkillBundle()` before upserting the skill by name.
 * The platform verifies the hash against the uploaded bytes and uses it for
 * deduplication.
 */
export interface BundledSkill {
  readonly zip: Uint8Array;
  readonly fileCount: number;
  readonly compressedSize: number;
}

const TEXT = new TextEncoder();

/** Inline files map: path -> contents (UTF-8 string or raw bytes). */
export type SkillFiles = Readonly<Record<string, string | Uint8Array>>;

/**
 * Fidelity metadata captured from a local directory walk: `exec` are the
 * bundle-relative paths that restore executable (0o755); `symlinks` are captured
 * links. When non-empty it is serialized into the {@link RESERVED_META_ENTRY}
 * sidecar, appended LAST so a pure-content bundle stays byte-identical to the
 * pre-fidelity output (dedup continuity).
 */
export interface BundleMeta {
  readonly exec?: readonly string[];
  readonly symlinks?: readonly BundleSymlink[];
}

export interface ParsedSkillBundle {
  readonly files: SkillFiles;
  readonly meta?: BundleMeta;
}

/** Remove and parse the reserved fidelity sidecar before canonical rebundling. */
export function splitSkillBundleMetadata(
  entries: Readonly<Record<string, Uint8Array>>,
  source: string
): ParsedSkillBundle {
  const files: Record<string, Uint8Array> = { ...entries };
  const sidecar = files[RESERVED_META_ENTRY];
  delete files[RESERVED_META_ENTRY];
  const collected = collectCanonicalArchiveFiles(files, source);
  if (sidecar === undefined) {
    validateBundleGraph(collected, undefined, source);
    return { files: Object.fromEntries(collected) };
  }
  const manifest = parseBundleManifest(sidecar);
  if (manifest === null) throw new Error(`${source}: invalid ${RESERVED_META_ENTRY} fidelity metadata`);
  const meta = validateBundleGraph(
    collected,
    { exec: manifest.exec, symlinks: manifest.symlinks },
    source
  );
  return { files: Object.fromEntries(collected), ...(meta !== undefined ? { meta } : {}) };
}

/**
 * Build the ordered {@link Zippable} for a collected content map: content entries
 * sorted lexicographically, then (when `meta` carries any exec/symlink) the
 * canonical `.aexmeta.json` sidecar appended LAST. Insertion order is the zip
 * order (fflate iterates keys in insertion order), so the sidecar is always the
 * final entry and a metadata-free bundle is byte-identical to today's output.
 */
function buildCanonicalZippable(
  collected: Map<string, Uint8Array>,
  meta?: BundleMeta
): { zippable: Zippable; sidecarBytes: number } {
  const sorted = [...collected.entries()].sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
  const zippable: Zippable = {};
  for (const [path, bytes] of sorted) {
    zippable[path] = [bytes, { mtime: ZIP_EPOCH }];
  }
  let sidecarBytes = 0;
  if (meta && !bundleManifestIsEmpty(meta)) {
    const sidecar = serializeBundleManifest({ v: 1, exec: meta.exec ?? [], symlinks: meta.symlinks ?? [] });
    sidecarBytes = sidecar.byteLength;
    zippable[RESERVED_META_ENTRY] = [sidecar, { mtime: ZIP_EPOCH }];
  }
  return { zippable, sidecarBytes };
}

/** Reject a user file colliding with the reserved sidecar name (matches the `tool.json` precedent). */
function assertNotReservedMetaPath(path: string, kind: string): void {
  if (path === RESERVED_META_ENTRY) {
    throw new Error(`${kind} files must not include reserved "${RESERVED_META_ENTRY}"; fidelity metadata is emitted by the SDK`);
  }
}

function collectCanonicalArchiveFiles(
  files: Readonly<Record<string, Uint8Array>>,
  source: string
): Map<string, Uint8Array> {
  const entries = Object.entries(files);
  assertArchiveEntryCount(entries.length, source);
  const collected = new Map<string, Uint8Array>();
  for (const [rawPath, bytes] of entries) {
    if (!(bytes instanceof Uint8Array)) {
      throw new Error(`${source} file ${JSON.stringify(rawPath)} must be a Uint8Array`);
    }
    const path = validateSkillBundleEntry({ path: rawPath, size: bytes.byteLength }).path;
    assertNotReservedMetaPath(path, source);
    if (collected.has(path)) throw new Error(`${source} contains duplicate path: ${path}`);
    collected.set(path, bytes);
  }
  return collected;
}

/** Validate the complete regular-file + fidelity-leaf graph before zipping. */
function validateBundleGraph(
  collected: ReadonlyMap<string, Uint8Array>,
  meta: BundleMeta | undefined,
  source: string
): BundleMeta | undefined {
  assertArchiveEntryCount(collected.size, source);
  const leaves = new Map<string, "regular file" | "symlink">();
  for (const path of collected.keys()) leaves.set(path, "regular file");

  if (meta === undefined) {
    assertNoLeafPrefixConflicts(leaves, source);
    return undefined;
  }
  if (meta === null || typeof meta !== "object" || Array.isArray(meta)) {
    throw new Error(`${source} fidelity metadata must be an object`);
  }
  const raw = meta as { readonly exec?: unknown; readonly symlinks?: unknown };
  if (raw.exec !== undefined && !Array.isArray(raw.exec)) {
    throw new Error(`${source} fidelity metadata exec must be an array`);
  }
  if (raw.symlinks !== undefined && !Array.isArray(raw.symlinks)) {
    throw new Error(`${source} fidelity metadata symlinks must be an array`);
  }
  const rawExec = raw.exec ?? [];
  const rawSymlinks = raw.symlinks ?? [];
  assertArchiveEntryCount(rawExec.length, `${source} executable metadata`);
  assertArchiveEntryCount(collected.size + rawSymlinks.length, source);

  const exec: string[] = [];
  const execSet = new Set<string>();
  for (const value of rawExec) {
    const path = canonicalMetadataPath(value, source, "executable");
    if (execSet.has(path)) throw new Error(`${source} contains duplicate executable path: ${path}`);
    if (!collected.has(path)) {
      throw new Error(`${source} executable path ${JSON.stringify(path)} must reference a regular file`);
    }
    execSet.add(path);
    exec.push(path);
  }

  const symlinks: BundleSymlink[] = [];
  for (const value of rawSymlinks) {
    if (!value || typeof value !== "object" || Array.isArray(value)) {
      throw new Error(`${source} fidelity metadata contains a malformed symlink`);
    }
    const record = value as { readonly path?: unknown; readonly target?: unknown };
    const path = canonicalMetadataPath(record.path, source, "symlink");
    if (typeof record.target !== "string") {
      throw new Error(`${source} symlink ${JSON.stringify(path)} target must be a string`);
    }
    if (record.target.length > MAX_SYMLINK_TARGET_LENGTH) {
      throw new Error(
        `${source} symlink ${JSON.stringify(path)} target exceeds ${MAX_SYMLINK_TARGET_LENGTH} characters`
      );
    }
    const conflict = leaves.get(path);
    if (conflict !== undefined) {
      throw new Error(`${source} symlink path ${JSON.stringify(path)} conflicts with a ${conflict}`);
    }
    leaves.set(path, "symlink");
    symlinks.push({ path, target: record.target });
  }

  assertNoLeafPrefixConflicts(leaves, source);
  return { exec, symlinks };
}

function canonicalMetadataPath(value: unknown, source: string, kind: "executable" | "symlink"): string {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`${source} ${kind} path must be a non-empty string`);
  }
  let path: string;
  try {
    path = validateSkillBundleEntry({ path: value, size: 0 }).path;
  } catch (error) {
    throw new Error(
      `${source} ${kind} path ${JSON.stringify(value)} is invalid: ` +
        `${error instanceof Error ? error.message : String(error)}`
    );
  }
  if (path === RESERVED_META_ENTRY) {
    throw new Error(`${source} ${kind} path must not use reserved ${JSON.stringify(RESERVED_META_ENTRY)}`);
  }
  return path;
}

function assertNoLeafPrefixConflicts(
  leaves: ReadonlyMap<string, "regular file" | "symlink">,
  source: string
): void {
  for (const path of leaves.keys()) {
    let separator = path.lastIndexOf("/");
    while (separator > 0) {
      const ancestor = path.slice(0, separator);
      if (leaves.has(ancestor)) {
        throw new Error(
          `${source} leaf prefix conflict: ${JSON.stringify(ancestor)} conflicts with ${JSON.stringify(path)}`
        );
      }
      separator = ancestor.lastIndexOf("/");
    }
  }
}

export function bundleSkillFiles(files: SkillFiles, meta?: BundleMeta): BundledSkill {
  if (!files || typeof files !== "object") {
    throw new Error("Skill files map is required");
  }
  const entries = Object.entries(files);
  if (entries.length === 0) {
    throw new Error("Skill files map cannot be empty");
  }
  if (entries.length > SKILL_BUNDLE_LIMITS.maxFiles) {
    throw new Error(`Skill bundle exceeds ${SKILL_BUNDLE_LIMITS.maxFiles} file limit (got ${entries.length})`);
  }

  const collected = new Map<string, Uint8Array>();
  let hasSkillMd = false;
  let totalDecompressed = 0;

  for (const [rawPath, contents] of entries) {
    const bytes = typeof contents === "string" ? TEXT.encode(contents) : contents;
    if (!(bytes instanceof Uint8Array)) {
      throw new Error(`Skill file "${rawPath}" must be a string or Uint8Array`);
    }
    const entry = validateSkillBundleEntry({ path: rawPath, size: bytes.byteLength });
    assertNotReservedMetaPath(entry.path, "Skill bundle");
    if (entry.path === "SKILL.md") {
      hasSkillMd = true;
    }
    totalDecompressed += bytes.byteLength;
    if (totalDecompressed > SKILL_BUNDLE_LIMITS.maxDecompressedBytes) {
      throw new Error(
        `Skill bundle exceeds decompressed cap of ${SKILL_BUNDLE_LIMITS.maxDecompressedBytes} bytes`
      );
    }
    if (collected.has(entry.path)) {
      throw new Error(`Skill bundle contains duplicate path: ${entry.path}`);
    }
    collected.set(entry.path, bytes);
  }

  if (!hasSkillMd) {
    throw new Error(
      'Skill bundle must contain a "SKILL.md" file at the root. ' +
        "If you want to upload an instructions file or generic agent context, " +
        "use Instructions.fromPath / File.fromPath instead."
    );
  }

  // Sort entries and pin every mtime to the epoch so the byte output is
  // identical across machines and re-runs (the BFF re-canonicalises and
  // recomputes the canonical hash, so this is for retry-safety / debug
  // reproducibility rather than a wire-shape contract). The fidelity sidecar,
  // when present, is appended LAST so a metadata-free bundle is byte-identical.
  const validatedMeta = validateBundleGraph(collected, meta, "Skill bundle");
  const { zippable, sidecarBytes } = buildCanonicalZippable(collected, validatedMeta);
  assertArchiveExpandedSize(totalDecompressed + sidecarBytes, "Skill bundle");

  const zip = zipSync(zippable, { level: 6 });
  assertArchiveCompressedSize(zip.byteLength, "Skill bundle");

  return { zip, fileCount: entries.length, compressedSize: zip.byteLength };
}

export interface BundledTool {
  readonly zip: Uint8Array;
  readonly fileCount: number;
  readonly compressedSize: number;
}

export interface ToolBundleManifest {
  readonly name: string;
  readonly description: string;
  readonly input_schema: ToolInputSchema;
  readonly entry: string;
}

export function bundleToolFiles(
  files: SkillFiles,
  manifest: ToolBundleManifest,
  meta?: BundleMeta
): BundledTool {
  if (!files || typeof files !== "object") {
    throw new Error("Tool files map is required");
  }
  const entries = Object.entries(files);
  if (entries.length === 0) {
    throw new Error("Tool files map cannot be empty");
  }
  if (entries.length > SKILL_BUNDLE_LIMITS.maxFiles) {
    throw new Error(`Tool bundle exceeds ${SKILL_BUNDLE_LIMITS.maxFiles} file limit (got ${entries.length})`);
  }

  const collected = new Map<string, Uint8Array>();
  let totalDecompressed = 0;
  let hasEntry = false;
  const entryPath = validateSkillBundleEntry({ path: manifest.entry, size: 0 }).path;

  for (const [rawPath, contents] of entries) {
    const bytes = typeof contents === "string" ? TEXT.encode(contents) : contents;
    if (!(bytes instanceof Uint8Array)) {
      throw new Error(`Tool file "${rawPath}" must be a string or Uint8Array`);
    }
    const entry = validateSkillBundleEntry({ path: rawPath, size: bytes.byteLength });
    assertNotReservedMetaPath(entry.path, "Tool bundle");
    totalDecompressed += bytes.byteLength;
    if (totalDecompressed > SKILL_BUNDLE_LIMITS.maxDecompressedBytes) {
      throw new Error(
        `Tool bundle exceeds decompressed cap of ${SKILL_BUNDLE_LIMITS.maxDecompressedBytes} bytes`
      );
    }
    if (collected.has(entry.path)) {
      throw new Error(`Tool bundle contains duplicate path: ${entry.path}`);
    }
    if (entry.path === entryPath) hasEntry = true;
    collected.set(entry.path, bytes);
  }

  if (!hasEntry) {
    throw new Error(`Tool bundle entry "${entryPath}" must exist in files`);
  }

  const manifestBytes = TEXT.encode(`${JSON.stringify(manifest, null, 2)}\n`);
  if (collected.has("tool.json")) {
    throw new Error('Tool bundle files must not include reserved "tool.json"; pass manifest fields to Tool.fromFiles instead');
  }
  collected.set("tool.json", manifestBytes);

  const validatedMeta = validateBundleGraph(collected, meta, "Tool bundle");
  const { zippable, sidecarBytes } = buildCanonicalZippable(collected, validatedMeta);
  assertArchiveExpandedSize(totalDecompressed + manifestBytes.byteLength + sidecarBytes, "Tool bundle");

  const zip = zipSync(zippable, { level: 6 });
  assertArchiveCompressedSize(zip.byteLength, "Tool bundle");

  return { zip, fileCount: collected.size, compressedSize: zip.byteLength };
}

const ZIP_EPOCH = new Date(Date.UTC(1980, 0, 1));

/**
 * Compute `sha256:<hex>` of the given canonicalised zip bytes. Used by the
 * `Skill.from*` / `File` / `Instructions` factories to populate the draft's
 * `contentHash` field. The hash is advisory — the BFF verifies
 * it against the uploaded zip; a mismatch is rejected. Web-Crypto-only so the
 * SDK works in Bun, Node, edge runtimes, and browsers without polyfills.
 */
export async function hashSkillBundle(zipBytes: Uint8Array): Promise<string> {
  const subtle = (globalThis as { crypto?: { subtle?: SubtleCrypto } }).crypto?.subtle;
  if (!subtle) {
    throw new Error(
      "hashSkillBundle: globalThis.crypto.subtle is not available; Bun, Node 18+, or a Web-Crypto-capable runtime is required"
    );
  }
  // crypto.subtle.digest expects a BufferSource. Pass a freshly-sliced
  // copy to detach from any external Uint8Array view (Web Crypto rejects
  // non-zero byteOffset SharedArrayBuffer views, and view detach also
  // protects against the caller mutating the input after the digest is
  // computed).
  const view = new Uint8Array(zipBytes.byteLength);
  view.set(zipBytes);
  const digest = await subtle.digest("SHA-256", view.buffer);
  return "sha256:" + bufferToHex(digest);
}

function bufferToHex(buffer: ArrayBuffer): string {
  const view = new Uint8Array(buffer);
  let out = "";
  for (let i = 0; i < view.length; i++) {
    const byte = view[i] as number;
    out += byte.toString(16).padStart(2, "0");
  }
  return out;
}
