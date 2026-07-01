import { unzipSync } from "fflate";
import { SKILL_BUNDLE_LIMITS, type FetchLike } from "@aexhq/contracts";
import type { SkillFiles } from "./bundle.js";

/**
 * Fetch a zip-archived skill from a URL and reduce it to the same in-memory
 * `SkillFiles` map that `Skill.fromFiles` consumes.
 *
 * This runs in the SDK process (the caller's own app), so the URL is
 * caller-controlled — there is no SSRF surface here. Host the skill yourself
 * and hand the SDK a temporary signed URL (e.g. an S3 presigned URL); it only
 * needs to be valid for this fetch, because `client.run` / `openSession`
 * snapshots the bytes into the run afterwards.
 *
 * Signed URLs carry secrets in their query string, so the query is never echoed
 * in error messages.
 */

const DEFAULT_TIMEOUT_MS = 30_000;

export interface FetchSkillArchiveOptions {
  /**
   * Optional integrity check over the downloaded archive bytes:
   * `sha256:<64 hex>` or bare hex. Verified before unzip; a mismatch throws.
   * This is a source-integrity check on the fetched archive — distinct from
   * the canonical bundle hash the SDK computes after re-canonicalising.
   */
  readonly sha256?: string;
  /** Abort the fetch after this many ms (default 30000). */
  readonly timeoutMs?: number;
  /** Injectable `fetch` (defaults to `globalThis.fetch`). */
  readonly fetch?: FetchLike;
}

export async function fetchSkillArchive(
  url: string,
  opts: FetchSkillArchiveOptions = {}
): Promise<SkillFiles> {
  if (typeof url !== "string" || url.length === 0) {
    throw new Error("Skill.fromUrl: url is required");
  }
  const fetchImpl = opts.fetch ?? (globalThis as { fetch?: FetchLike }).fetch;
  if (typeof fetchImpl !== "function") {
    throw new Error(
      "Skill.fromUrl: global fetch is unavailable; pass args.fetch " +
        "(Bun, Node 18+, or a fetch-capable runtime is required)"
    );
  }

  const bytes = await download(url, fetchImpl, opts.timeoutMs ?? DEFAULT_TIMEOUT_MS);
  if (opts.sha256) {
    await verifySha256(bytes, opts.sha256, url);
  }
  const entries = unzip(bytes, url);
  return resolveSkillRoot(entries, url);
}

// ---------------------------------------------------------------------------
// Download + integrity
// ---------------------------------------------------------------------------

async function download(url: string, fetchImpl: FetchLike, timeoutMs: number): Promise<Uint8Array> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  let res: Response;
  try {
    res = await fetchImpl(url, { signal: controller.signal });
  } catch (err) {
    throw new Error(`Skill.fromUrl: fetch failed for ${redactUrl(url)}: ${errMessage(err)}`);
  } finally {
    clearTimeout(timer);
  }
  if (!res.ok) {
    throw new Error(`Skill.fromUrl: fetch for ${redactUrl(url)} returned HTTP ${res.status}`);
  }
  // Early guard on a declared size so a clearly-too-big archive fails before
  // we buffer it. The authoritative caps are re-checked by bundleSkillFiles.
  const declared = Number(res.headers.get("content-length"));
  if (Number.isFinite(declared) && declared > SKILL_BUNDLE_LIMITS.maxCompressedBytes) {
    throw new Error(
      `Skill.fromUrl: archive at ${redactUrl(url)} declares ${declared} bytes, ` +
        `exceeding the ${SKILL_BUNDLE_LIMITS.maxCompressedBytes}-byte compressed cap`
    );
  }
  const bytes = new Uint8Array(await res.arrayBuffer());
  if (bytes.byteLength > SKILL_BUNDLE_LIMITS.maxCompressedBytes) {
    throw new Error(
      `Skill.fromUrl: archive at ${redactUrl(url)} is ${bytes.byteLength} bytes, ` +
        `exceeding the ${SKILL_BUNDLE_LIMITS.maxCompressedBytes}-byte compressed cap`
    );
  }
  if (bytes.byteLength === 0) {
    throw new Error(`Skill.fromUrl: archive at ${redactUrl(url)} is empty`);
  }
  return bytes;
}

async function verifySha256(bytes: Uint8Array, expected: string, url: string): Promise<void> {
  const want = (expected.startsWith("sha256:") ? expected.slice("sha256:".length) : expected).toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(want)) {
    throw new Error(
      `Skill.fromUrl: sha256 must be 64 hex chars (optionally prefixed "sha256:"), ` +
        `got ${JSON.stringify(expected)}`
    );
  }
  const got = await sha256Hex(bytes);
  if (got !== want) {
    throw new Error(
      `Skill.fromUrl: archive integrity check failed for ${redactUrl(url)}: ` +
        `expected sha256:${want} but downloaded bytes hash to sha256:${got}`
    );
  }
}

// ---------------------------------------------------------------------------
// Unzip + skill-root resolution
// ---------------------------------------------------------------------------

function unzip(bytes: Uint8Array, url: string): Record<string, Uint8Array> {
  try {
    return unzipSync(bytes);
  } catch (err) {
    throw new Error(
      `Skill.fromUrl: could not unzip the archive at ${redactUrl(url)} ` +
        `(expected a .zip): ${errMessage(err)}`
    );
  }
}

/**
 * Resolve the skill root from unzipped entries:
 *
 *  1. drop directory entries,
 *  2. if `SKILL.md` is already at root, use entries as-is,
 *  3. else if there is exactly one common top-level directory AND it contains
 *     `SKILL.md`, strip that single prefix,
 *  4. otherwise throw, listing the archive's actual top-level entries.
 *
 * We only ever strip a single level, and only when it demonstrably exposes a
 * root `SKILL.md` — never speculatively. `bundleSkillFiles` re-asserts the
 * SKILL.md-at-root invariant as the authoritative check.
 */
function resolveSkillRoot(entries: Record<string, Uint8Array>, url: string): SkillFiles {
  const files: Record<string, Uint8Array> = {};
  for (const [rawPath, bytes] of Object.entries(entries)) {
    const path = rawPath.replace(/\\/g, "/");
    if (path.endsWith("/")) {
      continue; // directory entry
    }
    files[path] = bytes;
  }

  const paths = Object.keys(files);
  if (paths.length === 0) {
    throw new Error(`Skill.fromUrl: archive at ${redactUrl(url)} contains no files`);
  }

  if (Object.prototype.hasOwnProperty.call(files, "SKILL.md")) {
    return files;
  }

  const topSegments = new Set(paths.map((p) => p.split("/")[0]));
  if (topSegments.size === 1) {
    const prefix = `${[...topSegments][0]}/`;
    if (Object.prototype.hasOwnProperty.call(files, `${prefix}SKILL.md`)) {
      const stripped: Record<string, Uint8Array> = {};
      for (const [p, b] of Object.entries(files)) {
        stripped[p.slice(prefix.length)] = b;
      }
      return stripped;
    }
  }

  const roots = [...new Set(paths.map((p) => (p.includes("/") ? `${p.split("/")[0]}/` : p)))].sort();
  throw new Error(
    `Skill.fromUrl: fetched archive at ${redactUrl(url)} must contain SKILL.md at its root, ` +
      `or inside a single top-level folder. Found top-level entries: ${roots.join(", ")}`
  );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Strip the query string so signed-URL secrets never reach error text/logs. */
function redactUrl(url: string): string {
  try {
    const u = new URL(url);
    return u.search ? `${u.origin}${u.pathname}?…` : `${u.origin}${u.pathname}`;
  } catch {
    const q = url.indexOf("?");
    return q >= 0 ? `${url.slice(0, q)}?…` : url;
  }
}

function errMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

async function sha256Hex(bytes: Uint8Array): Promise<string> {
  const subtle = (globalThis as { crypto?: { subtle?: SubtleCrypto } }).crypto?.subtle;
  if (!subtle) {
    throw new Error(
      "Skill.fromUrl: globalThis.crypto.subtle is unavailable; " +
        "Bun, Node 18+, or a Web-Crypto-capable runtime is required"
    );
  }
  const copy = new Uint8Array(bytes.byteLength);
  copy.set(bytes);
  const digest = await subtle.digest("SHA-256", copy.buffer);
  const view = new Uint8Array(digest);
  let out = "";
  for (let i = 0; i < view.length; i++) {
    out += (view[i] as number).toString(16).padStart(2, "0");
  }
  return out;
}
