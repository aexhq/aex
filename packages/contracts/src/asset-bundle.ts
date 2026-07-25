import { zipSync, type Zippable } from "fflate";
import {
  MAX_SYMLINK_TARGET_LENGTH,
  RESERVED_META_ENTRY,
  bundleManifestIsEmpty,
  tryParseBundleManifest,
  serializeBundleManifest,
  type BundleSymlink
} from "./bundle-manifest.js";
import {
  SKILL_BUNDLE_LIMITS,
  parseSkillBundleEntry,
  type ToolInputSchema
} from "./session-config.js";
import {
  assertArchiveCompressedSize,
  assertArchiveEntryCount,
  assertArchiveExpandedSize
} from "./archive-limits.js";
import {
  archiveFileBytesSchema,
  bundleFidelityMetaSchema,
  bundleFileContentsSchema,
  bundleFilesMapSchema,
  bundleMetadataPathSchema,
  bundleSymlinkRecordSchema,
  bundleSymlinkTargetSchema
} from "./schemas/asset-bundle.js";
import { parseWire } from "./schemas/wire.js";

/**
 * In-memory skill bundle: a flat path -> bytes map and the
 * deterministically-zipped representation.
 *
 * Public authoring consumers run the cheap, safety-critical checks here
 * (`parseSkillBundleEntry`: no `..`, no absolute paths, no Windows
 * backslashes, depth/length limits). The BFF re-canonicalises and
 * recomputes the canonical hash on receipt. Client-side hashing is advisory
 * and exists to make uploads retry-safe and content-addressable.
 *
 * For workspace skills the SDK and CLI compute an advisory `sha256` of the
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

/** Build the canonical single-entry archive used by File/Instructions drafts. */
export function bundleSingleFile(
  path: string,
  bytes: Uint8Array,
  source: string,
  enforceCompressedLimit = true
): Uint8Array {
  assertArchiveEntryCount(1, source);
  assertArchiveExpandedSize(bytes.byteLength, source);
  const zip = zipSync({ [path]: [bytes, { mtime: ZIP_EPOCH }] }, { level: 6 });
  if (enforceCompressedLimit) assertArchiveCompressedSize(zip.byteLength, source);
  return zip;
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
  const manifest = tryParseBundleManifest(sidecar);
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
    parseWire(archiveFileBytesSchema(source, rawPath), bytes);
    const path = parseSkillBundleEntry({ path: rawPath, size: bytes.byteLength }).path;
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
  const raw = parseWire(bundleFidelityMetaSchema(source), meta);
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
    parseWire(bundleSymlinkRecordSchema(source), value);
    const record = value as { readonly path?: unknown; readonly target?: unknown };
    const path = canonicalMetadataPath(record.path, source, "symlink");
    const target = parseWire(
      bundleSymlinkTargetSchema(source, path, MAX_SYMLINK_TARGET_LENGTH),
      record.target
    );
    const conflict = leaves.get(path);
    if (conflict !== undefined) {
      throw new Error(`${source} symlink path ${JSON.stringify(path)} conflicts with a ${conflict}`);
    }
    leaves.set(path, "symlink");
    symlinks.push({ path, target });
  }

  assertNoLeafPrefixConflicts(leaves, source);
  return { exec, symlinks };
}

function canonicalMetadataPath(value: unknown, source: string, kind: "executable" | "symlink"): string {
  const supplied = parseWire(bundleMetadataPathSchema(source, kind), value);
  let path: string;
  try {
    path = parseSkillBundleEntry({ path: supplied, size: 0 }).path;
  } catch (error) {
    throw new Error(
      `${source} ${kind} path ${JSON.stringify(supplied)} is invalid: ` +
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

type BundleKind = "Skill" | "Tool";

interface BundlePolicy<State> {
  readonly kind: BundleKind;
  readonly prepare: () => State;
  readonly visitEntry: (state: State, path: string) => void;
  readonly complete: (state: State, collected: Map<string, Uint8Array>) => number;
}

/**
 * Canonical pipeline shared by skill and tool authoring. Kind policies own only
 * their required-root checks and any generated regular file; entry coercion,
 * validation, accounting, collection, archive finalization, and counting stay
 * byte-for-byte coupled here.
 */
function bundleCanonicalFiles<State>(
  files: SkillFiles,
  meta: BundleMeta | undefined,
  diagnostics: "sdk" | "cli",
  policy: BundlePolicy<State>
): BundledSkill {
  const { kind } = policy;
  const source = `${kind} bundle`;
  parseWire(bundleFilesMapSchema(kind, source, SKILL_BUNDLE_LIMITS.maxFiles), files);
  const entries = Object.entries(files);

  const state = policy.prepare();
  const collected = new Map<string, Uint8Array>();
  let totalDecompressed = 0;
  for (const [rawPath, contents] of entries) {
    parseWire(bundleFileContentsSchema(kind, rawPath), contents);
    const bytes = typeof contents === "string" ? TEXT.encode(contents) : contents;
    const entry = parseSkillBundleEntry({ path: rawPath, size: bytes.byteLength });
    assertNotReservedMetaPath(entry.path, source);
    totalDecompressed += bytes.byteLength;
    if (totalDecompressed > SKILL_BUNDLE_LIMITS.maxDecompressedBytes) {
      throw new Error(
        `${source} exceeds decompressed cap of ${SKILL_BUNDLE_LIMITS.maxDecompressedBytes} bytes`
      );
    }
    if (collected.has(entry.path)) {
      throw new Error(`${source} contains duplicate path: ${entry.path}`);
    }
    policy.visitEntry(state, entry.path);
    collected.set(entry.path, bytes);
  }

  totalDecompressed += policy.complete(state, collected);
  const validatedMeta = validateBundleGraph(collected, meta, source);
  const { zippable, sidecarBytes } = buildCanonicalZippable(collected, validatedMeta);
  assertArchiveExpandedSize(totalDecompressed + sidecarBytes, source);

  const zip = zipSync(zippable, { level: 6 });
  assertBundleCompressedSize(zip.byteLength, source, diagnostics);
  return { zip, fileCount: collected.size, compressedSize: zip.byteLength };
}

export function bundleSkillFiles(
  files: SkillFiles,
  meta?: BundleMeta,
  diagnostics: "sdk" | "cli" = "sdk"
): BundledSkill {
  return bundleCanonicalFiles(files, meta, diagnostics, {
    kind: "Skill",
    prepare: () => ({ hasSkillMd: false }),
    visitEntry: (state, path) => {
      if (path === "SKILL.md") state.hasSkillMd = true;
    },
    complete: (state) => {
      if (!state.hasSkillMd) {
        throw new Error(
          'Skill bundle must contain a "SKILL.md" file at the root. ' +
            "If you want to upload an instructions file or generic agent context, " +
            "use Instructions.fromPath / File.fromPath instead."
        );
      }
      return 0;
    }
  });
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
  meta?: BundleMeta,
  diagnostics: "sdk" | "cli" = "sdk"
): BundledTool {
  return bundleCanonicalFiles(files, meta, diagnostics, {
    kind: "Tool",
    prepare: () => ({
      entryPath: parseSkillBundleEntry({ path: manifest.entry, size: 0 }).path,
      hasEntry: false
    }),
    visitEntry: (state, path) => {
      if (path === state.entryPath) state.hasEntry = true;
    },
    complete: (state, collected) => {
      if (!state.hasEntry) {
        throw new Error(`Tool bundle entry "${state.entryPath}" must exist in files`);
      }

      const manifestBytes = TEXT.encode(`${JSON.stringify(manifest, null, 2)}\n`);
      if (collected.has("tool.json")) {
        throw new Error('Tool bundle files must not include reserved "tool.json"; pass manifest fields to Tool.fromFiles instead');
      }
      collected.set("tool.json", manifestBytes);
      return manifestBytes.byteLength;
    }
  });
}

const ZIP_EPOCH = new Date(Date.UTC(1980, 0, 1));

/**
 * Compute `sha256:<hex>` of the given canonicalised zip bytes. Used by the
 * `Skill.from*` / `File` / `Instructions` factories to populate the draft's
 * `contentHash` field. The hash is advisory — the BFF verifies
 * it against the uploaded zip; a mismatch is rejected. Web-Crypto-only so the
 * SDK works in Bun, Node, edge runtimes, and browsers without polyfills.
 */
export async function hashSkillBundle(
  zipBytes: Uint8Array,
  diagnostics: "sdk" | "cli" = "sdk"
): Promise<string> {
  const subtle = (globalThis as { crypto?: { subtle?: SubtleCrypto } }).crypto?.subtle;
  if (!subtle) {
    if (diagnostics === "cli") {
      throw new Error("sha256: globalThis.crypto.subtle is not available");
    }
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

function assertBundleCompressedSize(size: number, source: string, diagnostics: "sdk" | "cli"): void {
  if (diagnostics === "cli" && size > SKILL_BUNDLE_LIMITS.maxCompressedBytes) {
    throw new Error(`bundle exceeds compressed cap of ${SKILL_BUNDLE_LIMITS.maxCompressedBytes} bytes (got ${size})`);
  }
  assertArchiveCompressedSize(size, source);
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
