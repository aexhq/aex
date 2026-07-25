/**
 * Session-request config and composition refs for the public SDK/CLI surface.
 *
 * Public composition concepts:
 *
 *   - `SkillRef` is the PUBLIC wire-level reference to a workspace skill: just
 *     `{ kind:"skill", name }`. The binding is BY NAME and mutable — the session
 *     resolves the name to the workspace skill's current bytes at submit time.
 *     It travels in `submission.skills` (NOT `submission.tools`).
 *
 *   - `McpServerRef` is the non-secret part of an MCP server declaration:
 *     `name` and `url`. Bearer / cookie / per-request headers travel in
 *     the session's vaulted `secrets.mcpServers` block keyed by the same
 *     `name`, and never enter the hashed submission payload or the
 *     session snapshot.
 *
 *   - `SessionRequestConfig` is the credential-free set of session parameters that
 *     can be persisted to disk (e.g. `aex start --config session.json`) or
 *     returned from ordinary application helper functions. It excludes
 *     `secrets`/`idempotencyKey`/`signal`; strings are already resolved at
 *     the call site before submission.
 *
 *   - Skill bundle validation lives here so the SDK (zipping locally),
 *     hosted API (server-side unzip + manifest extraction) and runtime
 *     mount layer share a single source of truth for
 *     the limits, the path normaliser, and the manifest invariants. The
 *     DB CHECK constraints on `skill_bundles.manifest` mirror these.
 *
 * Keep this as the public source of truth for the SDK/CLI composition
 * boundary.
 */

import {
  type JsonValue,
  type PlatformEnvironment
} from "./submission.js";
import { rethrowContractParseError, withContractParseError } from "./contract-parse-error.js";
import { type ModelName } from "./models.js";
import type { RuntimeSize } from "./runtime-sizes.js";
import {
  ASSET_ID_PATTERN,
  MOUNT_PATH_MAX_LENGTH,
  MOUNT_PATH_PATTERN,
  assertValidMountPath,
  normalizeAssetRef,
  parseAssetRefWire
} from "./schemas/asset-ref.js";
import {
  MCP_SERVER_NAME_PATTERN,
  REMOTE_MCP_STDIO_REJECTED_MESSAGE,
  normalizeMcpServerRef,
  parseMcpServerRefWire,
  rejectStdioMcpShape,
  type McpWirePolicy
} from "./schemas/mcp-server.js";
import { parseSessionRequestConfigWire } from "./schemas/session-request-config.js";

// ---------------------------------------------------------------------------
// Skill ID + name format
// ---------------------------------------------------------------------------

/**
 * Mirrors the server-side CHECK constraint
 * `skill_bundles_id_format_chk = check (id ~ '^skl_[A-Za-z0-9_-]{8,128}$')`
 * on persisted skill bundles. Keep the two in lockstep.
 */
export const SKILL_ID_PATTERN = /^skl_[A-Za-z0-9_-]{8,128}$/;

/**
 * Human-readable, workspace-scoped name. Lowercase, kebab-friendly,
 * 1..128 chars. The DB enforces the length bound via
 * `skill_bundles_name_len_chk`; this regex tightens the SDK/CLI input
 * surface so callers fail at the boundary rather than in the BFF.
 */
export const SKILL_NAME_PATTERN = /^[a-z0-9][a-z0-9_-]{0,127}$/;

/**
 * Provider-safe submitted tool name. Tool names share the same lowercase
 * kebab/underscore envelope as skills. Submission parsing rejects `__`
 * because SessionDO reserves that separator for MCP namespace routing.
 */
export const TOOL_NAME_PATTERN = SKILL_NAME_PATTERN;

/**
 * Names reserved by the skills subsystem and therefore usable as neither a
 * skill name nor a custom tool name. `skills` is the injected meta-tool (see
 * {@link SKILLS_TOOL_NAME} in `submission.ts`); `skill` is its singular. Both
 * the SDK factories and the BFF `parseSkills` / `parseTools` reject these.
 */
export const SKILL_RESERVED_NAMES: ReadonlySet<string> = new Set(["skills", "skill"]);

// ---------------------------------------------------------------------------
// Runtime asset archive limits
// ---------------------------------------------------------------------------

/**
 * One honest envelope for every file, skill, tool, or instruction archive that
 * the managed runtime mounts. Raw asset storage is a broader concept and may
 * have a different quota; these limits describe usable runtime inputs.
 */
export const ASSET_ARCHIVE_LIMITS = {
  maxCompressedBytes: 64 * 1024 * 1024,
  maxDecompressedBytes: 128 * 1024 * 1024,
  maxEntries: 1_000,
  maxMetadataBytes: 8 * 1024 * 1024
} as const;

/** Skill-specific authoring constraints layered on the shared asset envelope. */
export const SKILL_BUNDLE_LIMITS = {
  /** Compressed (.zip) ceiling. */
  maxCompressedBytes: ASSET_ARCHIVE_LIMITS.maxCompressedBytes,
  /** Sum of uncompressed file sizes. */
  maxDecompressedBytes: ASSET_ARCHIVE_LIMITS.maxDecompressedBytes,
  /** Number of regular file entries (directories don't count). */
  maxFiles: ASSET_ARCHIVE_LIMITS.maxEntries,
  /** Maximum directory nesting depth — `a/b/c/d` has depth 4. */
  maxDepth: 16,
  /** Single-entry path length cap. */
  maxPathLength: 512,
  /** Stored file mode for ordinary files. */
  defaultFileMode: 0o644,
  /** Stored directory mode. */
  defaultDirMode: 0o755
} as const;

// ---------------------------------------------------------------------------
// By-name SkillRef used by the standalone skill bundle helpers.
// ---------------------------------------------------------------------------

/**
 * The PUBLIC wire reference to a workspace skill — by NAME, no bytes, no hash.
 * This is what the SDK sends in `submission.skills` and what the idempotency
 * hash covers. The session resolves the name to the workspace skill's CURRENT bytes
 * at submit time (a re-upload under the same name changes what later sessions see).
 */
export interface SkillRef {
  readonly kind: "skill";
  readonly name: string; // SKILL_NAME_PATTERN, must not contain "__", not reserved
}

/**
 * Storage-neutral uploaded asset reference. Runtime materialization resolves
 * `assetId` privately; public callers never name object-store paths.
 */
export interface AssetRef {
  readonly kind: "asset";
  readonly assetId: string;
  readonly name: string;
  readonly mountPath?: string;
}

type ToolJsonPrimitive = string | number | boolean | null;
type ToolJsonValue = ToolJsonPrimitive | ToolJsonValue[] | { readonly [key: string]: ToolJsonValue };
export type ToolInputSchema = { readonly [key: string]: ToolJsonValue };

/**
 * User-supplied executable tool bundle. The bytes are addressed by the same
 * content-addressed asset ref used by Skills/Files, while the provider-visible
 * manifest rides as value-free metadata on the submission.
 */
export interface ToolRef extends AssetRef {
  readonly description: string;
  readonly input_schema: ToolInputSchema;
  readonly entry: string;
}

/** Content-hash format: `sha256:<64 lowercase hex>`. */
export {
  CANONICAL_SHA256_DIGEST_PATTERN as INLINE_CONTENT_HASH_PATTERN
} from "./canonical-sha256.js";

/**
 * Checks the `AssetRef` discriminator on an already typed file reference.
 *
 * This compatibility predicate is not full validation for an untrusted value;
 * use the owning parser when the complete asset-ref shape must be validated.
 */
export function isAssetRef(ref: FileRef): ref is AssetRef {
  return ref.kind === "asset";
}

/**
 * Asset ids are storage-neutral product ids. Current uploads derive the id from
 * the content digest (`asset_<sha256hex>`), but callers must treat it as opaque.
 *
 * Declared in `schemas/asset-ref.ts` next to the schema that enforces it — this
 * module imports that one, so it cannot live here without a cycle.
 */
export { ASSET_ID_PATTERN };

// ---------------------------------------------------------------------------
// File refs — third SDK concept. Uploaded assets carry the DIRECTORY the
// managed runtime unzips them into (`mountPath`), and the real on-disk
// `filename` to preserve inside that directory.
// ---------------------------------------------------------------------------

export type FileRef = AssetRef;

/** @deprecated Use `isAssetRef`. This direct alias remains for public compatibility. */
export const isFileAssetRef: typeof isAssetRef = isAssetRef;

/**
 * The default mount DIRECTORY a `File` unzips into when the caller does not set
 * `mountPath`. `/workspace` is also the agent's default working directory, so a
 * file handed with no `mountPath` lands directly in the agent's cwd (e.g.
 * `/workspace/source-video-subtitles.srt`).
 */
export const DEFAULT_FILE_MOUNT_PATH = "/workspace";

/**
 * A `mountPath` is an ABSOLUTE container directory under the workspace, and
 * {@link assertValidMountPath} is the assert the SDK `File` builders and the BFF
 * asset-ref parser share so both reject the same malformed input.
 *
 * All three are declared in `schemas/asset-ref.ts`, next to the schema that
 * enforces them — this module imports that one, so they cannot live here without
 * a cycle.
 */
export { MOUNT_PATH_PATTERN, MOUNT_PATH_MAX_LENGTH, assertValidMountPath };

/**
 * Common parser for any `kind: "asset"` ref (file / tool bundle).
 */
export function parseAssetRefFields(
  record: Record<string, unknown>,
  path: string
): AssetRef {
  return withContractParseError("parseAssetRefFields", () =>
    normalizeAssetRef(parseAssetRefWire(record, path))
  );
}

// ---------------------------------------------------------------------------
// Skill bundle manifest + validation
// ---------------------------------------------------------------------------

/**
 * Manifest entry persisted in `skill_bundles.manifest` and in session-owned
 * snapshots. `path` is forward-slash, relative, normalised. `mode` is the
 * stored POSIX mode (sanitised, NOT the user's filesystem mode) — see
 * `SKILL_BUNDLE_LIMITS.defaultFileMode`.
 */
export interface SkillBundleEntry {
  readonly path: string;
  readonly size: number;
  readonly mode: number;
}

export interface SkillBundleManifest {
  readonly entries: readonly SkillBundleEntry[];
  /** Total uncompressed bytes (sum of `entries[i].size`). */
  readonly totalSize: number;
  /** Number of file entries. Equals `entries.length` by construction. */
  readonly fileCount: number;
}

export class SkillBundleValidationError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "SkillBundleValidationError";
  }
}

/**
 * Reject input paths that try to escape the bundle root or smuggle
 * platform-specific syntax. Returns the canonical forward-slash
 * relative path; never returns paths starting or ending with `/`.
 *
 * Rejects:
 *   - empty strings and pure whitespace
 *   - absolute paths (`/foo`, `C:\foo`, `\\server\share`)
 *   - backslash separators (Windows)
 *   - `..` segments anywhere in the path
 *   - `.` segments anywhere except a leading bare `.`
 *   - paths whose length exceeds `SKILL_BUNDLE_LIMITS.maxPathLength`
 *   - paths whose depth exceeds `SKILL_BUNDLE_LIMITS.maxDepth`
 *   - NUL bytes
 */
export function normaliseSkillBundlePath(input: string): string {
  if (typeof input !== "string") {
    throw new SkillBundleValidationError("bundle entry path must be a string");
  }
  if (input.length === 0 || input.trim().length === 0) {
    throw new SkillBundleValidationError("bundle entry path must be non-empty");
  }
  if (input.length > SKILL_BUNDLE_LIMITS.maxPathLength) {
    throw new SkillBundleValidationError(
      `bundle entry path exceeds maxPathLength (${SKILL_BUNDLE_LIMITS.maxPathLength}): ${input}`
    );
  }
  if (input.includes("\0")) {
    throw new SkillBundleValidationError(`bundle entry path contains NUL byte: ${JSON.stringify(input)}`);
  }
  if (input.includes("\\")) {
    throw new SkillBundleValidationError(`bundle entry path uses backslash separator: ${input}`);
  }
  if (/^[A-Za-z]:[\\/]/.test(input)) {
    throw new SkillBundleValidationError(`bundle entry path uses a drive letter: ${input}`);
  }
  if (input.startsWith("/")) {
    throw new SkillBundleValidationError(`bundle entry path must be relative: ${input}`);
  }

  // Reject trailing slash so callers cannot disguise directory entries
  // as files. The manifest is files-only.
  if (input.endsWith("/")) {
    throw new SkillBundleValidationError(`bundle entry path must not end with '/': ${input}`);
  }

  const segments = input.split("/");
  for (const segment of segments) {
    if (segment === ".." ) {
      throw new SkillBundleValidationError(`bundle entry path contains '..' segment: ${input}`);
    }
    if (segment === "." || segment === "") {
      throw new SkillBundleValidationError(`bundle entry path contains empty or '.' segment: ${input}`);
    }
  }
  if (segments.length > SKILL_BUNDLE_LIMITS.maxDepth) {
    throw new SkillBundleValidationError(
      `bundle entry path exceeds maxDepth (${SKILL_BUNDLE_LIMITS.maxDepth}): ${input}`
    );
  }
  return input;
}

/**
 * Validate one manifest entry: normalises the path, bounds the size,
 * and sanitises the mode to one of {defaultFileMode, defaultDirMode}.
 * The bundle is files-only, so any non-regular-file entry is rejected
 * upstream by the caller (zip parser must skip symlinks, device files,
 * etc. before reaching this function).
 */
export function parseSkillBundleEntry(input: {
  readonly path: string;
  readonly size: number;
  readonly mode?: number;
}): SkillBundleEntry {
  try {
    const path = normaliseSkillBundlePath(input.path);
    if (!Number.isFinite(input.size) || !Number.isInteger(input.size) || input.size < 0) {
      throw new SkillBundleValidationError(`bundle entry size must be a non-negative integer (${path})`);
    }
    if (input.size > SKILL_BUNDLE_LIMITS.maxDecompressedBytes) {
      throw new SkillBundleValidationError(
        `bundle entry size exceeds maxDecompressedBytes (${SKILL_BUNDLE_LIMITS.maxDecompressedBytes}): ${path}`
      );
    }
    // Sanitise the stored mode. Executable bit is implied by runtime
    // convention; we never persist arbitrary chmod from the user's FS.
    const mode = (input.mode ?? SKILL_BUNDLE_LIMITS.defaultFileMode) & 0o777;
    if (mode !== SKILL_BUNDLE_LIMITS.defaultFileMode && mode !== SKILL_BUNDLE_LIMITS.defaultDirMode) {
      return { path, size: input.size, mode: SKILL_BUNDLE_LIMITS.defaultFileMode };
    }
    return { path, size: input.size, mode };
  } catch (error) {
    rethrowContractParseError(error, "parseSkillBundleEntry");
  }
}

/** @deprecated Use {@link parseSkillBundleEntry}; this compatibility wrapper is identical. */
export function validateSkillBundleEntry(input: {
  readonly path: string;
  readonly size: number;
  readonly mode?: number;
}): SkillBundleEntry {
  return parseSkillBundleEntry(input);
}

/**
 * Validate a full **skill bundle** manifest. Enforces:
 *   - entries is a non-empty array
 *   - `SKILL.md` exists at the bundle root (this is what makes a
 *     bundle a skill rather than a plain workspace file)
 *   - file count <= maxFiles
 *   - total uncompressed size <= maxDecompressedBytes
 *   - per-entry validation (see `validateSkillBundleEntry`)
 *   - no duplicate paths
 *
 * In this public surface, **skill** means "Claude Skill" — bundles without
 * `SKILL.md` are not skills and must go
 * through workspace instructions or file uploads instead.
 *
 * Returns a canonical manifest with totals computed.
 */
export function parseSkillBundleManifest(
  input: ReadonlyArray<{ readonly path: string; readonly size: number; readonly mode?: number }>
): SkillBundleManifest {
  try {
    if (!Array.isArray(input) || input.length === 0) {
      throw new SkillBundleValidationError("bundle manifest must be a non-empty array of entries");
    }
    if (input.length > SKILL_BUNDLE_LIMITS.maxFiles) {
      throw new SkillBundleValidationError(
        `bundle exceeds maxFiles (${SKILL_BUNDLE_LIMITS.maxFiles}): got ${input.length}`
      );
    }
    const seen = new Set<string>();
    const entries: SkillBundleEntry[] = [];
    let totalSize = 0;
    let hasSkillMd = false;
    for (const raw of input) {
      const entry = parseSkillBundleEntry(raw);
      if (seen.has(entry.path)) {
        throw new SkillBundleValidationError(`bundle manifest contains duplicate path: ${entry.path}`);
      }
      seen.add(entry.path);
      if (entry.path === "SKILL.md") {
        hasSkillMd = true;
      }
      totalSize += entry.size;
      if (totalSize > SKILL_BUNDLE_LIMITS.maxDecompressedBytes) {
        throw new SkillBundleValidationError(
          `bundle total size exceeds maxDecompressedBytes (${SKILL_BUNDLE_LIMITS.maxDecompressedBytes})`
        );
      }
      entries.push(entry);
    }
    if (!hasSkillMd) {
      throw new SkillBundleValidationError(
        "skill bundle manifest must contain a 'SKILL.md' entry at the bundle root. " +
          "If you want to upload an instructions file or generic agent context, use " +
          "workspace instructions or File instead."
      );
    }
    return { entries, totalSize, fileCount: entries.length };
  } catch (error) {
    rethrowContractParseError(error, "parseSkillBundleManifest");
  }
}

/** @deprecated Use {@link parseSkillBundleManifest}; this compatibility wrapper is identical. */
export function validateSkillBundleManifest(
  input: ReadonlyArray<{ readonly path: string; readonly size: number; readonly mode?: number }>
): SkillBundleManifest {
  return parseSkillBundleManifest(input);
}

/**
 * Returns true when the manifest carries a `SKILL.md` entry at the
 * bundle root. The presence of this file is Anthropic's
 * skill-auto-discovery signal — bundles that have it are treated as
 * Claude skills and mounted accordingly; bundles that don't are still
 * usable agent context (AGENTS.md, settings files, folders of helper
 * data) but the agent won't pick them up via the skills mechanism.
 *
 * The check is intentionally a separate, callable predicate (rather
 * than baked into `validateSkillBundleManifest`) so the storage and
 * the attach layers can remain independent.
 */
export function hasSkillMdAtRoot(manifest: SkillBundleManifest): boolean {
  return manifest.entries.some((entry) => entry.path === "SKILL.md");
}

// ---------------------------------------------------------------------------
// McpServerRef (non-secret) + SessionConfigMcpServer (with optional headers)
// ---------------------------------------------------------------------------

/**
 * Remote MCP transports Aex accepts. Both are over HTTP — `http`
 * is the streamable-HTTP transport, `sse` is the event-stream
 * transport. `stdio` is explicitly NOT a value here: local-process
 * MCP is not implemented.
 */
export const REMOTE_MCP_TRANSPORTS = ["http", "sse"] as const;
export type RemoteMcpTransport = (typeof REMOTE_MCP_TRANSPORTS)[number];

/**
 * Canonical error string for any attempt to declare a stdio-shaped MCP
 * server (`transport: "stdio"`, or a stdio-only field like `command` /
 * `args` / `env`). Pinned in source so every surface — shared parser,
 * SDK builder, CLI flag parser, dashboard form — surfaces the same
 * message and a user can find it via grep.
 *
 * Declared in `schemas/mcp-server.ts` alongside the shape gate that raises it.
 */
export { REMOTE_MCP_STDIO_REJECTED_MESSAGE };

/**
 * The non-secret half of an MCP server declaration. This is what enters
 * the hashed submission, the session snapshot, and any audit log. `name`
 * keys into `secrets.mcpServers` for the per-request headers.
 *
 * `transport` is optional on the wire — when omitted, the runtime is
 * free to pick the default remote transport (`http`). When present, it
 * MUST be one of {@link REMOTE_MCP_TRANSPORTS}; stdio is rejected at
 * parse time.
 */
export interface McpServerRef {
  readonly name: string;
  readonly url: string;
  readonly transport?: RemoteMcpTransport;
}

/** Declared in `schemas/mcp-server.ts`, next to the schema that enforces it. */
export { MCP_SERVER_NAME_PATTERN };

/**
 * A session-config MCP entry. The user is free to supply headers inline; the SDK
 * splits the call site cleanly at submission time so the Authorization (or
 * other auth-bearing) header never enters the non-secret wire payload.
 */
export interface SessionConfigMcpServer extends McpServerRef {
  readonly headers?: Readonly<Record<string, string>>;
}

/**
 * Parse the non-secret half of an MCP server declaration mounted at `path`.
 *
 * SSRF guard at the parser boundary (C4) — the platform MCP proxy relies on
 * this validation; each branch of {@link denyReasonForMcpHost} maps to an SSRF
 * regression test case.
 */
export function parseMcpServerRef(input: unknown, path: string): McpServerRef {
  return withContractParseError("parseMcpServerRef", () =>
    normalizeMcpServerRef(parseMcpServerRefWire(input, path, MCP_WIRE_POLICY))
  );
}

export { rejectStdioMcpShape };

/**
 * Reasons an IP-literal host should be refused. Returns null when the
 * literal is a routable public address (or not an IP literal at all — name
 * resolution is the caller's concern). This numeric-range deny-list is kept
 * in parity across the public contract parser and platform shared parser so
 * the MCP parser, the egress proxy handlers, and `submission.parseProxyBaseUrl`
 * classify the same bytes.
 *
 * `host` is the already-bracket-stripped, lowercased hostname.
 *
 * NOTE — residual DNS-rebind gap: this denies IP *literals* only. A name
 * that resolves to a private/metadata IP is NOT caught here (we don't
 * resolve at parse time). Closing that requires resolve-then-pin at egress;
 * that pinning is deferred. The host's outbound fetch still refuses RFC1918
 * at connect, but not loopback/169.254/CGNAT/ULA — which is exactly why the
 * literal checks below exist as defense in depth.
 */
function denyReasonForHostIp(host: string): string | null {
  // IPv4-mapped / IPv4-compatible IPv6 literals decode to an embedded IPv4 —
  // classify that IPv4 so a mapped form can't smuggle a private target. Two
  // shapes reach us: the dotted-quad form a caller may type (`::ffff:127.0.0.1`)
  // and the hex form `new URL().hostname` normalises it to (`::ffff:7f00:1`),
  // where the two trailing hextets ARE the four IPv4 octets.
  const mappedDotted = /^::(?:ffff:)?(\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})$/.exec(host);
  if (mappedDotted) {
    return denyReasonForV4(mappedDotted[1]!);
  }
  const mappedHex = /^::ffff:([0-9a-f]{1,4}):([0-9a-f]{1,4})$/.exec(host);
  if (mappedHex) {
    const hi = Number.parseInt(mappedHex[1]!, 16);
    const lo = Number.parseInt(mappedHex[2]!, 16);
    const dotted = `${hi >> 8}.${hi & 0xff}.${lo >> 8}.${lo & 0xff}`;
    return denyReasonForV4(dotted);
  }
  const v4 = denyReasonForV4(host);
  if (v4) return v4;
  // Unspecified IPv6 cannot identify a routable remote endpoint.
  if (host === "::") {
    return "must not target unspecified IPv6 (::)";
  }
  // Loopback IPv6 (::1 in any acceptable form)
  if (host === "::1" || host === "0:0:0:0:0:0:0:1") {
    return "must not target loopback IPv6 (::1)";
  }
  // Link-local IPv6 (fe80::/10 — fe80:: through febf::)
  if (/^fe[89ab][0-9a-f]?:/.test(host)) {
    return "must not target link-local IPv6 (fe80::/10)";
  }
  // Unique-local IPv6 (fc00::/7 — fc00:: through fdff::), the IPv6
  // equivalent of RFC1918 private space.
  if (/^f[cd][0-9a-f]{0,2}:/.test(host)) {
    return "must not target unique-local IPv6 (fc00::/7)";
  }
  return null;
}

/**
 * IPv4-literal deny-list. Returns null when `host` is not a dotted-quad or
 * is a routable public IPv4. Split out of {@link denyReasonForHostIp} so the
 * IPv4-mapped IPv6 branch reuses the exact same ranges.
 */
function denyReasonForV4(host: string): string | null {
  if (!/^\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(host)) return null;
  const octets = host.split(".").map((o) => Number.parseInt(o, 10));
  if (octets.some((o) => o > 255)) return null;
  const [a, b] = octets as [number, number, number, number];
  // Unspecified / current-network IPv4 is not a routable remote target.
  if (a === 0) return "must not target unroutable IPv4 (0.0.0.0/8)";
  // Loopback IPv4 (127.0.0.0/8)
  if (a === 127) return "must not target loopback IPv4 (127.0.0.0/8)";
  // Link-local / metadata IPv4 (169.254.0.0/16 — includes 169.254.169.254)
  if (a === 169 && b === 254) {
    return "must not target link-local IPv4 (169.254.0.0/16) — cloud metadata range";
  }
  // CGNAT shared address space (100.64.0.0/10) — runners NAT through it, so
  // an upstream there can reach sibling tenants / the runner host.
  if (a === 100 && b >= 64 && b <= 127) {
    return "must not target CGNAT IPv4 (100.64.0.0/10)";
  }
  // Benchmarking range (198.18.0.0/15) is reserved for inter-network tests.
  if (a === 198 && (b === 18 || b === 19)) {
    return "must not target benchmark IPv4 (198.18.0.0/15)";
  }
  // RFC1918 private ranges (10/8, 172.16/12, 192.168/16) — defense in depth.
  if (a === 10) return "must not target RFC1918 IPv4 (10.0.0.0/8)";
  if (a === 172 && b >= 16 && b <= 31) return "must not target RFC1918 IPv4 (172.16.0.0/12)";
  if (a === 192 && b === 168) return "must not target RFC1918 IPv4 (192.168.0.0/16)";
  // Multicast, reserved, and limited-broadcast space is never a public HTTP target.
  if (a >= 224) return "must not target multicast/reserved IPv4 (224.0.0.0/3)";
  return null;
}

/**
 * Reasons an MCP server URL should be refused at parse time. Returns null
 * when the URL is acceptable. Hostnames are lowercased; numeric ranges
 * are checked literally so the catch covers both names ("localhost") and
 * IP literals ("127.0.0.1") symmetrically. The numeric-range checks
 * delegate to {@link denyReasonForHostIp} (shared with the platform proxy).
 *
 * Surface tracked by server-side SSRF regression coverage.
 */
function denyReasonForMcpHost(parsed: URL): string | null {
  // `new URL("https://[fe80::1]/").hostname` returns `[fe80::1]` WITH
  // the brackets on Node 22; strip them so the IPv6 checks match either
  // shape symmetrically.
  const host = parsed.hostname.toLowerCase().replace(/^\[|\]$/g, "");
  // Loopback name (covers `localhost` + `localhost.localdomain` etc.)
  if (host === "localhost" || host.endsWith(".localhost")) {
    return "must not target a loopback hostname";
  }
  const ipDenial = denyReasonForHostIp(host);
  if (ipDenial) return ipDenial;
  // Port constraint: https must be on 443 (defense in depth — non-standard
  // https ports often indicate internal services). http allowance keeps
  // the existing local-dev pattern (e.g. host.docker.internal:8787) usable.
  if (parsed.protocol === "https:" && parsed.port !== "" && parsed.port !== "443") {
    return `must use port 443 for https (got ${parsed.port})`;
  }
  return null;
}

function parseRemoteMcpTransport(input: unknown, field: string): RemoteMcpTransport | undefined {
  if (input === undefined) {
    return undefined;
  }
  if (typeof input !== "string" || !(REMOTE_MCP_TRANSPORTS as readonly string[]).includes(input)) {
    throw new Error(
      `${field} must be one of: ${REMOTE_MCP_TRANSPORTS.join(", ")} (got ${JSON.stringify(input)})`
    );
  }
  return input as RemoteMcpTransport;
}

/**
 * The two rules the MCP schemas defer back to this module for.
 *
 * Injected rather than imported the other way around because
 * `scripts/cicd/check-contract-parity.mjs` locates the SSRF deny-list INSIDE
 * this file — it slices from `denyReasonForHostIp` to `parseRemoteMcpTransport`
 * and byte-compares that region against `platform/packages/shared/src/blueprint.ts`.
 * Moving either function into `schemas/` would break the only check that keeps
 * the two deny-lists identical.
 */
const MCP_WIRE_POLICY: McpWirePolicy = {
  denyReasonForHost: denyReasonForMcpHost,
  parseTransport: parseRemoteMcpTransport
};

// ---------------------------------------------------------------------------
// Session request config
// ---------------------------------------------------------------------------

/**
 * Plain JSON accepted by `aex start --config <path>`. This is not a
 * platform object; it is only the non-secret session parameters that the CLI folds
 * into the normal `submit` request.
 */
export interface SessionRequestConfig {
  readonly model: ModelName;
  readonly system?: string;
  readonly prompt: string | readonly string[];
  readonly mcpServers?: readonly SessionConfigMcpServer[];
  readonly environment?: PlatformEnvironment;
  /** Managed runtime size preset (see {@link RuntimeSize}). */
  readonly runtimeSize?: RuntimeSize;
  /** Session deadline as a duration string (`"1h"`, `"30m"`); bounded [1m, 8h] server-side. */
  readonly timeout?: string;
  readonly metadata?: Readonly<Record<string, JsonValue>>;
}

// ---------------------------------------------------------------------------
// Session request config parser (used by CLI to load `session.json`)
// ---------------------------------------------------------------------------

/**
 * Parse a session request config from JSON. Defensive — used by the host CLI to
 * load `--config session.json`. Throws with the JSON path that failed so
 * a user can fix their file. Headers are preserved here and split out
 * later by the SDK normalisation step.
 */
export function parseSessionRequestConfig(input: unknown): SessionRequestConfig {
  return withContractParseError("parseSessionRequestConfig", () =>
    parseSessionRequestConfigWire(input, MCP_WIRE_POLICY)
  );
}

// ---------------------------------------------------------------------------
// Normalisation: session config -> wire-ready submission + secrets split
// ---------------------------------------------------------------------------

/**
 * Result of splitting a session config into the non-secret submission and the
 * secret MCP-headers bundle. The SDK calls this just before posting to
 * /api/sessions: the `submission` half is what the BFF hashes for idempotency, the
 * `mcpServerSecrets` half is what enters session-scoped custody.
 *
 * `prompt` is normalised to `readonly string[]` (single-string callers
 * get wrapped in a length-1 array) so the wire payload, the hosted API,
 * and the audit log don't have to re-handle two shapes.
 */
export interface NormalisedSessionRequestConfig {
  readonly model: ModelName;
  readonly system?: string;
  readonly prompt: readonly string[];
  readonly mcpServers: readonly McpServerRef[];
  readonly environment?: PlatformEnvironment;
  readonly metadata?: Readonly<Record<string, JsonValue>>;
  /**
   * MCP servers whose session-config entry carried `headers`. Keyed by the `name`
   * that appears in `mcpServers` so the BFF can pair them up.
   */
  readonly mcpServerSecrets: ReadonlyArray<{
    readonly name: string;
    readonly url: string;
    readonly headers: Readonly<Record<string, string>>;
  }>;
}

export function normaliseSessionRequestConfig(config: SessionRequestConfig): NormalisedSessionRequestConfig {
  const prompt: readonly string[] =
    typeof config.prompt === "string" ? [config.prompt] : config.prompt;
  const mcpServers: McpServerRef[] = [];
  const mcpServerSecrets: NormalisedSessionRequestConfig["mcpServerSecrets"][number][] = [];
  for (const entry of config.mcpServers ?? []) {
    mcpServers.push({ name: entry.name, url: entry.url });
    if (entry.headers !== undefined) {
      mcpServerSecrets.push({ name: entry.name, url: entry.url, headers: entry.headers });
    }
  }
  return {
    model: config.model,
    ...(config.system !== undefined ? { system: config.system } : {}),
    prompt,
    mcpServers,
    ...(config.environment !== undefined ? { environment: config.environment } : {}),
    ...(config.metadata !== undefined ? { metadata: config.metadata } : {}),
    mcpServerSecrets
  };
}
