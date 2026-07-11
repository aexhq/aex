import { zipSync, type Zippable } from "fflate";
import {
  RESERVED_META_ENTRY,
  SKILL_BUNDLE_LIMITS,
  bundleManifestIsEmpty,
  serializeBundleManifest,
  validateSkillBundleEntry,
  type BundleSymlink,
  type ToolInputSchema
} from "@aexhq/contracts";

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

/**
 * Build the ordered {@link Zippable} for a collected content map: content entries
 * sorted lexicographically, then (when `meta` carries any exec/symlink) the
 * canonical `.aexmeta.json` sidecar appended LAST. Insertion order is the zip
 * order (fflate iterates keys in insertion order), so the sidecar is always the
 * final entry and a metadata-free bundle is byte-identical to today's output.
 */
function buildCanonicalZippable(collected: Map<string, Uint8Array>, meta?: BundleMeta): Zippable {
  const sorted = [...collected.entries()].sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
  const zippable: Zippable = {};
  for (const [path, bytes] of sorted) {
    zippable[path] = [bytes, { mtime: ZIP_EPOCH }];
  }
  if (meta && !bundleManifestIsEmpty(meta)) {
    const sidecar = serializeBundleManifest({ v: 1, exec: meta.exec ?? [], symlinks: meta.symlinks ?? [] });
    zippable[RESERVED_META_ENTRY] = [sidecar, { mtime: ZIP_EPOCH }];
  }
  return zippable;
}

/** Reject a user file colliding with the reserved sidecar name (matches the `tool.json` precedent). */
function assertNotReservedMetaPath(path: string, kind: string): void {
  if (path === RESERVED_META_ENTRY) {
    throw new Error(`${kind} files must not include reserved "${RESERVED_META_ENTRY}"; fidelity metadata is emitted by the SDK`);
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
  const zippable = buildCanonicalZippable(collected, meta);

  const zip = zipSync(zippable, { level: 6 });
  if (zip.byteLength > SKILL_BUNDLE_LIMITS.maxCompressedBytes) {
    throw new Error(
      `Skill bundle exceeds compressed cap of ${SKILL_BUNDLE_LIMITS.maxCompressedBytes} bytes (got ${zip.byteLength})`
    );
  }

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

  const zippable = buildCanonicalZippable(collected, meta);

  const zip = zipSync(zippable, { level: 6 });
  if (zip.byteLength > SKILL_BUNDLE_LIMITS.maxCompressedBytes) {
    throw new Error(
      `Tool bundle exceeds compressed cap of ${SKILL_BUNDLE_LIMITS.maxCompressedBytes} bytes (got ${zip.byteLength})`
    );
  }

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
