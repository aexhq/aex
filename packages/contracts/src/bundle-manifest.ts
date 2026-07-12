/**
 * Bundle fidelity sidecar — the shared source of truth for the `.aexmeta.json`
 * metadata entry that rides inside a canonical bundle zip.
 *
 * A canonical bundle (skill / tool / file / instruction) is a deterministic zip of
 * regular files. fflate's `unzipSync` on the restore side is asymmetric: it
 * surfaces only entry bytes, never external attributes / os field / symlink
 * flags. So exec bits and symlinks — the metadata a local dir → zip →
 * container-restore round-trip must preserve — cannot travel in the zip's
 * external attributes and be read back. Instead they travel in ONE reserved
 * entry, `.aexmeta.json`, which both the SDK (encode) and the container
 * materializer (restore) agree on. This module is that agreement.
 *
 * SECURITY: {@link symlinkTargetEscapes} is a restore-time boundary — it decides
 * whether a captured symlink target may be recreated on the container FS. It is
 * PURELY LEXICAL (no realpath, no FS access → no TOCTOU) and is fuzzed. Both
 * repos import it; do not fork it.
 *
 * Determinism: the sidecar is emitted ONLY when there is metadata to carry (exec
 * or symlinks non-empty) and is serialized canonically (fixed field order, sorted
 * arrays, no insignificant whitespace, no trailing newline). A pure-content
 * bundle emits NO sidecar → its bytes stay identical to the pre-fidelity output,
 * so existing content-addressed dedup is preserved.
 */

import { ASSET_ARCHIVE_LIMITS } from "./session-config.js";

/** Reserved zip entry carrying the fidelity sidecar. Rejected as a user file path. */
export const RESERVED_META_ENTRY = ".aexmeta.json";

/** Normalised mode for an executable regular file (has any exec bit). */
export const EXEC_MODE = 0o755;
/** Normalised mode for an ordinary regular file. */
export const DEFAULT_FILE_MODE = 0o644;

/** Bundle-time cap on a captured symlink target string. */
export const MAX_SYMLINK_TARGET_LENGTH = 4096;
/** Defensive cap for each metadata array before the parser iterates it. */
const MAX_BUNDLE_METADATA_RECORDS = ASSET_ARCHIVE_LIMITS.maxEntries;

/** A captured symlink: `path` is the bundle-relative link name, `target` the raw `readlink()` string. */
export interface BundleSymlink {
  readonly path: string;
  readonly target: string;
}

/**
 * The fidelity sidecar. `exec` lists the bundle-relative paths that restore to
 * {@link EXEC_MODE} (all other files stay {@link DEFAULT_FILE_MODE}); `symlinks`
 * lists links to recreate (subject to {@link symlinkTargetEscapes}).
 */
export interface BundleManifest {
  readonly v: 1;
  readonly exec: readonly string[];
  readonly symlinks: readonly BundleSymlink[];
}

/** True when a manifest carries no metadata worth emitting (so no sidecar is written). */
export function bundleManifestIsEmpty(manifest: {
  readonly exec?: readonly string[];
  readonly symlinks?: readonly { readonly path: string }[];
}): boolean {
  return (manifest.exec?.length ?? 0) === 0 && (manifest.symlinks?.length ?? 0) === 0;
}

const META_TEXT_ENCODER = new TextEncoder();
const META_TEXT_DECODER = new TextDecoder("utf-8", { fatal: true });

/**
 * Serialize a manifest to canonical, byte-stable UTF-8 bytes: fixed field order
 * (`v`, `exec`, `symlinks`), `exec` sorted ascending, `symlinks` sorted by
 * `path`, no insignificant whitespace (compact JSON), no trailing newline. Two
 * equal manifests always serialize to identical bytes → the sidecar never
 * perturbs dedup determinism.
 */
export function serializeBundleManifest(manifest: BundleManifest): Uint8Array {
  if (manifest.exec.length > MAX_BUNDLE_METADATA_RECORDS || manifest.symlinks.length > MAX_BUNDLE_METADATA_RECORDS) {
    throw new Error(`bundle fidelity metadata exceeds the ${MAX_BUNDLE_METADATA_RECORDS}-record limit`);
  }
  if (manifest.symlinks.some((entry) => entry.target.length > MAX_SYMLINK_TARGET_LENGTH)) {
    throw new Error(`bundle fidelity symlink target exceeds ${MAX_SYMLINK_TARGET_LENGTH} characters`);
  }
  const exec = [...manifest.exec].sort(byString);
  const symlinks = [...manifest.symlinks]
    .map((s) => ({ path: s.path, target: s.target }))
    .sort((a, b) => byString(a.path, b.path));
  // Object literal key order is the wire order under JSON.stringify (fixed).
  const json = JSON.stringify({ v: 1, exec, symlinks });
  const bytes = META_TEXT_ENCODER.encode(json);
  if (bytes.byteLength > ASSET_ARCHIVE_LIMITS.maxMetadataBytes) {
    throw new Error("bundle fidelity metadata exceeds the 8 MiB limit");
  }
  return bytes;
}

/**
 * Parse the sidecar bytes into a validated {@link BundleManifest}, or `null` when
 * the bytes are absent, not valid canonical JSON, or carry an unknown version.
 * Forward-compatible: an unknown `v` yields `null`. A known-v1 manifest is
 * atomic: every `exec` and `symlinks` entry must be structurally valid or the
 * whole sidecar returns `null`, so callers cannot silently materialize a partial
 * fidelity graph. The parse never throws.
 */
export function parseBundleManifest(bytes: Uint8Array | null | undefined): BundleManifest | null {
  if (!bytes || bytes.byteLength === 0) return null;
  if (bytes.byteLength > ASSET_ARCHIVE_LIMITS.maxMetadataBytes) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(META_TEXT_DECODER.decode(bytes));
  } catch {
    return null;
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return null;
  const record = parsed as Record<string, unknown>;
  if (record.v !== 1) return null;
  if (!Array.isArray(record.exec) || !Array.isArray(record.symlinks)) return null;
  if (
    record.exec.length > MAX_BUNDLE_METADATA_RECORDS ||
    record.symlinks.length > MAX_BUNDLE_METADATA_RECORDS
  ) return null;
  const exec: string[] = [];
  for (const item of record.exec) {
    if (typeof item !== "string" || item.length === 0) return null;
    exec.push(item);
  }
  const symlinks: BundleSymlink[] = [];
  for (const item of record.symlinks) {
    if (!item || typeof item !== "object" || Array.isArray(item)) return null;
    const rec = item as Record<string, unknown>;
    const path = rec.path;
    const target = rec.target;
    if (
      typeof path !== "string" ||
      path.length === 0 ||
      typeof target !== "string" ||
      target.length > MAX_SYMLINK_TARGET_LENGTH
    ) return null;
    symlinks.push({ path, target });
  }
  return { v: 1, exec, symlinks };
}

/**
 * SECURITY boundary — decide whether recreating a symlink `linkRelPath` → `target`
 * would let the link escape the bundle root. PURELY LEXICAL (no realpath / no FS
 * access → no TOCTOU). Returns `true` (REJECT / do not create) when ANY of:
 *
 *   - `target` is empty or contains a NUL;
 *   - `target` is absolute — starts with `/`, matches a Windows drive (`C:`), is a
 *     UNC path (`\\host`), or contains any backslash;
 *   - the lexically-resolved target (join the link's dirname with the target, then
 *     normalise `.`/`..`) climbs at or above the bundle root (leading `..`).
 *
 * Otherwise the link is relative and resolves to a path inside (or at) the bundle
 * root — safe to recreate with the ORIGINAL relative `target`. A dangling in-root
 * target (pointing at a not-yet-existing sibling) is SAFE and allowed.
 */
export function symlinkTargetEscapes(linkRelPath: string, target: string): boolean {
  if (typeof target !== "string" || target.length === 0) return true;
  if (target.includes("\0")) return true;
  // Absolute / platform-specific targets are always rejected.
  if (target.startsWith("/")) return true;
  if (/^[A-Za-z]:/.test(target)) return true;
  if (target.includes("\\")) return true;

  // Lexically resolve relative to the link's own directory.
  const linkDir = posixDirname(linkRelPath);
  const joined = linkDir.length > 0 ? `${linkDir}/${target}` : target;
  const stack: string[] = [];
  for (const segment of joined.split("/")) {
    if (segment === "" || segment === ".") continue;
    if (segment === "..") {
      // Pop an in-root component, or record an escaping climb.
      if (stack.length > 0 && stack[stack.length - 1] !== "..") {
        stack.pop();
      } else {
        stack.push("..");
      }
    } else {
      stack.push(segment);
    }
  }
  // A leading `..` means the resolved path is at or above the bundle root.
  return stack.length > 0 && stack[0] === "..";
}

/** POSIX dirname of a forward-slash relative path ("" when there is no parent segment). */
function posixDirname(path: string): string {
  const idx = path.lastIndexOf("/");
  return idx <= 0 ? "" : path.slice(0, idx);
}

function byString(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}
