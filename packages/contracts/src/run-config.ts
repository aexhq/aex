/**
 * Run-request config and composition refs for the public SDK/CLI surface.
 *
 * Public composition concepts:
 *
 *   - `SkillRef` is the wire-level reference to a skill — either an
 *     `skl_*` id pointing at a workspace-uploaded bundle, a
 *     `{vendor, skillId, version}` reference to a provider built-in, or
 *     a `{slot, name, contentHash}` reference to per-run bytes attached
 *     as a multipart part on the submitRun call (and torn down at run
 *     terminal). The three shapes are discriminated by `kind` so
 *     consumers branch mechanically and providers can never accidentally
 *     be looked up in `skill_bundles`. Transient refs do NOT round-trip
 *     through JSON (bytes can't be serialised back); `parseRunRequestConfig`
 *     therefore rejects them while the BFF multipart submission parser
 *     accepts them.
 *
 *   - `McpServerRef` is the non-secret part of an MCP server declaration:
 *     `name` and `url`. Bearer / cookie / per-request headers travel in
 *     the run's vaulted `secrets.mcpServers` block keyed by the same
 *     `name`, and never enter the hashed submission payload or the
 *     run snapshot.
 *
 *   - `RunRequestConfig` is the credential-free set of run parameters that
 *     can be persisted to disk (e.g. `antpath run --config run.json`) or
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

import type {
  JsonValue,
  PlatformCleanupPolicy,
  PlatformProxyEndpoint,
  PlatformEnvironment
} from "./submission.js";
import type { RuntimeSize } from "./runtime-sizes.js";

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

// ---------------------------------------------------------------------------
// Skill bundle limits (uploaded bundles)
// ---------------------------------------------------------------------------

/**
 * Hard caps applied at upload time. The SDK enforces these before
 * computing the zip hash so a clearly-too-big bundle never wastes
 * bytes-on-the-wire; the BFF re-enforces server-side because the SDK
 * is untrusted. Numbers are deliberately conservative for the MVP and
 * can be tuned later; keep this object as the single tuning point.
 */
export const SKILL_BUNDLE_LIMITS = {
  /** Compressed (.zip) ceiling. */
  maxCompressedBytes: 10 * 1024 * 1024,
  /** Sum of uncompressed file sizes. */
  maxDecompressedBytes: 50 * 1024 * 1024,
  /** Number of regular file entries (directories don't count). */
  maxFiles: 1000,
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
// SkillRef (discriminated)
// ---------------------------------------------------------------------------

export type SkillRef = ProviderSkillRef | AssetRef;

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

export interface ProviderSkillRef {
  readonly kind: "provider";
  readonly vendor: "anthropic" | "custom";
  readonly skillId: string;
  readonly version?: string;
}

/** Content-hash format: `sha256:<64 lowercase hex>`. */
export const INLINE_CONTENT_HASH_PATTERN = /^sha256:[0-9a-f]{64}$/;

export function isProviderSkillRef(ref: SkillRef): ref is ProviderSkillRef {
  return ref.kind === "provider";
}

export function isAssetRef(ref: SkillRef | AgentsMdRef | FileRef): ref is AssetRef {
  return ref.kind === "asset";
}

/**
 * Asset ids are storage-neutral product ids. Current uploads derive the id from
 * the content digest (`asset_<sha256hex>`), but callers must treat it as opaque.
 */
export const ASSET_ID_PATTERN = /^asset_[A-Za-z0-9_-]{8,128}$/;

// ---------------------------------------------------------------------------
// AgentsMd refs — the second of the three SDK concepts.
// Stored on `workspace_files` with kind='agentsmd', `amd_*` ids.
// Attach mechanism: prepended as the first user message in the
// session (matches Claude Code's CLAUDE.md behaviour).
// AgentsMd is prepended as run-scoped instruction context.
// ---------------------------------------------------------------------------

export type AgentsMdRef = AssetRef;

export function isAgentsMdAssetRef(ref: AgentsMdRef): ref is AssetRef {
  return ref.kind === "asset";
}

// ---------------------------------------------------------------------------
// File refs — third SDK concept. Uploaded assets can carry a requested mount
// path for the managed runtime.
// ---------------------------------------------------------------------------

export type FileRef = AssetRef;

export function isFileAssetRef(ref: FileRef): ref is AssetRef {
  return ref.kind === "asset";
}

/**
 * Parse a `SkillRef` from untrusted input. Used by the BFF run parser
 * and by the operations module when deserialising API responses. Only
 * `kind: "asset"` and `kind: "provider"` are valid; all other historical
 * wire shapes (including storage-specific refs) are rejected.
 */
export function parseSkillRef(input: unknown, path: string): SkillRef {
  if (input === null || typeof input !== "object" || Array.isArray(input)) {
    throw new Error(`${path} must be a SkillRef object`);
  }
  const record = input as Record<string, unknown>;
  const kind = record.kind;
  if (kind === "provider") {
    for (const key of Object.keys(record)) {
      if (key !== "kind" && key !== "vendor" && key !== "skillId" && key !== "version") {
        throw new Error(`${path} contains unexpected field for provider SkillRef: ${key}`);
      }
    }
    const vendor = record.vendor;
    if (vendor !== "anthropic" && vendor !== "custom") {
      throw new Error(`${path}.vendor must be 'anthropic' or 'custom'`);
    }
    const skillId = record.skillId;
    if (typeof skillId !== "string" || skillId.length === 0 || skillId.length > 256) {
      throw new Error(`${path}.skillId must be a non-empty string (<= 256 chars)`);
    }
    const version = record.version;
    if (version !== undefined && (typeof version !== "string" || version.length === 0 || version.length > 64)) {
      throw new Error(`${path}.version, when provided, must be a non-empty string (<= 64 chars)`);
    }
    return {
      kind: "provider",
      vendor,
      skillId,
      ...(version !== undefined ? { version } : {})
    };
  }
  if (kind === "asset") {
    return parseAssetRefFields(record, path);
  }
  throw new Error(`${path}.kind must be 'provider' or 'asset'`);
}

/**
 * Common parser for any `kind: "asset"` ref (skill / agentsMd / file).
 */
export function parseAssetRefFields(
  record: Record<string, unknown>,
  path: string
): AssetRef {
  for (const key of Object.keys(record)) {
    if (
      key !== "kind" &&
      key !== "assetId" &&
      key !== "name" &&
      key !== "mountPath"
    ) {
      throw new Error(`${path} contains unexpected field for asset ref: ${key}`);
    }
  }
  const assetId = record.assetId;
  if (typeof assetId !== "string" || !ASSET_ID_PATTERN.test(assetId)) {
    throw new Error(`${path}.assetId must match ${ASSET_ID_PATTERN.source}`);
  }
  const name = record.name;
  if (typeof name !== "string" || name.length === 0 || name.length > 128) {
    throw new Error(`${path}.name must be a non-empty string (<= 128 chars)`);
  }
  const mountPath = record.mountPath;
  if (mountPath !== undefined && (typeof mountPath !== "string" || mountPath.length === 0)) {
    throw new Error(`${path}.mountPath, when provided, must be a non-empty string`);
  }
  return {
    kind: "asset",
    assetId,
    name,
    ...(mountPath !== undefined ? { mountPath } : {})
  };
}

// ---------------------------------------------------------------------------
// Skill bundle manifest + validation
// ---------------------------------------------------------------------------

/**
 * Manifest entry persisted in `skill_bundles.manifest` and
 * `run_skill_snapshots.manifest`. `path` is forward-slash, relative,
 * normalised. `mode` is the stored POSIX mode (sanitised, NOT the user's
 * filesystem mode) — see `SKILL_BUNDLE_LIMITS.defaultFileMode`.
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
export function validateSkillBundleEntry(input: {
  readonly path: string;
  readonly size: number;
  readonly mode?: number;
}): SkillBundleEntry {
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
 * through the `AgentsMd` or `File` upload concepts instead.
 *
 * Returns a canonical manifest with totals computed.
 */
export function validateSkillBundleManifest(
  input: ReadonlyArray<{ readonly path: string; readonly size: number; readonly mode?: number }>
): SkillBundleManifest {
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
    const entry = validateSkillBundleEntry(raw);
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
        "AgentsMd or File instead."
    );
  }
  return { entries, totalSize, fileCount: entries.length };
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
// McpServerRef (non-secret) + RunConfigMcpServer (with optional headers)
// ---------------------------------------------------------------------------

/**
 * Remote MCP transports Antpath accepts. Both are over HTTP — `http`
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
 */
export const REMOTE_MCP_STDIO_REJECTED_MESSAGE =
  "stdio MCP servers are not supported by Antpath. Antpath supports remote MCP servers over HTTP/SSE only.";

/**
 * Stdio-only fields. Used by the parser to detect a stdio shape even
 * when the caller omits `transport: "stdio"` (e.g. `{ url, command }`
 * — the presence of `command` alone is enough to identify a stdio
 * declaration and reject it).
 */
const STDIO_ONLY_FIELDS = ["command", "args", "env"] as const;

/**
 * The non-secret half of an MCP server declaration. This is what enters
 * the hashed submission, the run snapshot, and any audit log. `name`
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

export const MCP_SERVER_NAME_PATTERN = /^[a-z][a-z0-9_-]{0,62}$/;

/**
 * A run-config MCP entry. The user is free to supply headers inline; the SDK
 * splits the call site cleanly at submission time so the Authorization (or
 * other auth-bearing) header never enters the non-secret wire payload.
 */
export interface RunConfigMcpServer extends McpServerRef {
  readonly headers?: Readonly<Record<string, string>>;
}

export function parseMcpServerRef(input: unknown, path: string): McpServerRef {
  if (input === null || typeof input !== "object" || Array.isArray(input)) {
    throw new Error(`${path} must be an object`);
  }
  const record = input as Record<string, unknown>;
  rejectStdioMcpShape(record);
  // Headers belong on `RunConfigMcpServer`, not the non-secret wire ref;
  // and the wire `submission.mcpServers` must NEVER contain headers. So
  // reject any field other than {name,url,transport} explicitly to make a
  // caller accidentally inlining `headers` into the non-secret half fail
  // loudly instead of silently dropping the field.
  // `parseRunConfigMcpServerRef` handles the headers case separately for
  // run-config entries.
  for (const key of Object.keys(record)) {
    if (key !== "name" && key !== "url" && key !== "transport") {
      throw new Error(
        `${path}.${key} is not an allowed field for McpServerRef; permitted: name, url, transport`
      );
    }
  }
  const name = record.name;
  if (typeof name !== "string" || !MCP_SERVER_NAME_PATTERN.test(name)) {
    throw new Error(`${path}.name must match ${MCP_SERVER_NAME_PATTERN.source}`);
  }
  const url = record.url;
  if (typeof url !== "string" || url.length === 0) {
    throw new Error(`${path}.url must be a non-empty string`);
  }
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== "https:" && parsed.protocol !== "http:") {
      throw new Error(`${path}.url must use http or https (got ${parsed.protocol})`);
    }
    // Auth belongs in `secrets.mcpServers[i].headers`, never in the URL
    // itself. A `https://user:pass@host` style URL would be persisted in
    // the non-secret run snapshot and hashed into the idempotency key —
    // both unacceptable for credential material.
    if (parsed.username !== "" || parsed.password !== "") {
      throw new Error(
        `${path}.url must not contain userinfo (username/password); use secrets.mcpServers[].headers for auth`
      );
    }
    // SSRF guard at the parser boundary (C4) — the Worker MCP proxy
    // relies on this validation; CF's outbound fetch refuses RFC1918 but
    // does NOT block loopback names, link-local IPv6, 169.254.x, or
    // arbitrary non-443 ports on https. Each branch below maps to a
    // c4-worker-ssrf.regression test case.
    const ssrfDenial = denyReasonForMcpHost(parsed);
    if (ssrfDenial !== null) {
      throw new Error(`${path}.url ${ssrfDenial}`);
    }
  } catch (cause) {
    if (cause instanceof Error && cause.message.startsWith(path)) {
      throw cause;
    }
    throw new Error(`${path}.url is not a valid URL: ${url}`);
  }
  const transport = parseRemoteMcpTransport(record.transport, `${path}.transport`);
  return transport ? { name, url, transport } : { name, url };
}

/**
 * Throw the canonical stdio-rejected error if the record carries any
 * stdio-only marker (`transport: "stdio"`, `command`, `args`, `env`).
 * Used by both the shared parser and the SDK `McpServer.remote`
 * builder so every entry point surfaces the same message.
 */
export function rejectStdioMcpShape(record: Record<string, unknown>): void {
  if (record.transport === "stdio") {
    throw new Error(REMOTE_MCP_STDIO_REJECTED_MESSAGE);
  }
  for (const field of STDIO_ONLY_FIELDS) {
    if (record[field] !== undefined) {
      throw new Error(REMOTE_MCP_STDIO_REJECTED_MESSAGE);
    }
  }
}

/**
 * Reasons an MCP server URL should be refused at parse time. Returns null
 * when the URL is acceptable. Hostnames are lowercased; numeric ranges
 * are checked literally so the catch covers both names ("localhost") and
 * IP literals ("127.0.0.1") symmetrically.
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
  // Loopback IPv4 (127.0.0.0/8)
  if (/^127(?:\.[0-9]+){3}$/.test(host)) {
    return "must not target loopback IPv4 (127.0.0.0/8)";
  }
  // Loopback IPv6 (::1 in any acceptable form)
  if (host === "::1" || host === "0:0:0:0:0:0:0:1") {
    return "must not target loopback IPv6 (::1)";
  }
  // Link-local IPv6 (fe80::/10 — fe80:: through febf::)
  if (/^fe[89ab][0-9a-f]?:/.test(host)) {
    return "must not target link-local IPv6 (fe80::/10)";
  }
  // Link-local / metadata IPv4 (169.254.0.0/16 — includes 169.254.169.254)
  if (/^169\.254\.[0-9]+\.[0-9]+$/.test(host)) {
    return "must not target link-local IPv4 (169.254.0.0/16) — cloud metadata range";
  }
  // RFC1918 private ranges (10/8, 172.16/12, 192.168/16) — defense in depth.
  if (/^10\.[0-9]+\.[0-9]+\.[0-9]+$/.test(host)) {
    return "must not target RFC1918 IPv4 (10.0.0.0/8)";
  }
  if (/^172\.(1[6-9]|2[0-9]|3[01])\.[0-9]+\.[0-9]+$/.test(host)) {
    return "must not target RFC1918 IPv4 (172.16.0.0/12)";
  }
  if (/^192\.168\.[0-9]+\.[0-9]+$/.test(host)) {
    return "must not target RFC1918 IPv4 (192.168.0.0/16)";
  }
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
 * Strict parser for run-config MCP server entries. Allows only the
 * `{name, url, headers?}` shape so config loaded from `--config run.json`
 * cannot smuggle unrelated fields past the parser.
 */
function parseRunConfigMcpServerRef(input: unknown, path: string): RunConfigMcpServer {
  if (input === null || typeof input !== "object" || Array.isArray(input)) {
    throw new Error(`${path} must be an object`);
  }
  const record = input as Record<string, unknown>;
  rejectStdioMcpShape(record);
  for (const key of Object.keys(record)) {
    if (key !== "name" && key !== "url" && key !== "headers" && key !== "transport") {
      throw new Error(
        `${path}.${key} is not an allowed field for RunConfigMcpServer; permitted: name, url, transport, headers`
      );
    }
  }
  // Reuse the {name,url,transport} validator by passing the stripped object.
  const stripped: Record<string, unknown> = { name: record.name, url: record.url };
  if (record.transport !== undefined) stripped.transport = record.transport;
  const ref = parseMcpServerRef(stripped, path);
  const rawHeaders = record.headers;
  if (rawHeaders === undefined) {
    return ref;
  }
  if (rawHeaders === null || typeof rawHeaders !== "object" || Array.isArray(rawHeaders)) {
    throw new Error(`${path}.headers, when provided, must be a string-keyed object`);
  }
  const headers: Record<string, string> = {};
  for (const [hk, hv] of Object.entries(rawHeaders as Record<string, unknown>)) {
    if (typeof hv !== "string") {
      throw new Error(`${path}.headers.${hk} must be a string`);
    }
    headers[hk] = hv;
  }
  return { ...ref, headers };
}

// ---------------------------------------------------------------------------
// Run request config + migration aliases
// ---------------------------------------------------------------------------

/**
 * Plain JSON accepted by `antpath run --config <path>`. This is not a
 * platform object; it is only the non-secret run parameters that the CLI folds
 * into the normal `submitRun` request.
 */
export interface RunRequestConfig {
  readonly model: string;
  readonly system?: string;
  readonly prompt: string | readonly string[];
  readonly skills?: readonly SkillRef[];
  readonly mcpServers?: readonly RunConfigMcpServer[];
  readonly environment?: PlatformEnvironment;
  readonly cleanup?: PlatformCleanupPolicy;
  /** Managed runtime size preset (see {@link RuntimeSize}). */
  readonly runtimeSize?: RuntimeSize;
  /** Run deadline as a duration string (`"1h"`, `"30m"`); bounded [1m, 6h] server-side. */
  readonly timeout?: string;
  readonly proxyEndpoints?: readonly PlatformProxyEndpoint[];
  readonly metadata?: Readonly<Record<string, JsonValue>>;
}

// ---------------------------------------------------------------------------
// Run request config parser (used by CLI to load `run.json`)
// ---------------------------------------------------------------------------

/**
 * Parse a run request config from JSON. Defensive — used by the host CLI to
 * load `--config run.json`. Throws with the JSON path that failed so
 * a user can fix their file. Headers are preserved here and split out
 * later by the SDK normalisation step.
 */
export function parseRunRequestConfig(input: unknown): RunRequestConfig {
  if (input === null || typeof input !== "object" || Array.isArray(input)) {
    throw new Error("run request config must be an object");
  }
  const record = input as Record<string, unknown>;
  const allowed = new Set([
    "model",
    "system",
    "prompt",
    "skills",
    "mcpServers",
    "environment",
    "cleanup",
    "runtimeSize",
    "timeout",
    "proxyEndpoints",
    "metadata"
  ]);
  for (const key of Object.keys(record)) {
    if (!allowed.has(key)) {
      throw new Error(`run request config contains unexpected field: ${key}`);
    }
  }
  const model = record.model;
  if (typeof model !== "string" || model.length === 0) {
    throw new Error("run request config model must be a non-empty string");
  }
  const system = record.system;
  if (system !== undefined && typeof system !== "string") {
    throw new Error("run request config system, when provided, must be a string");
  }
  const prompt = parseRunRequestConfigPrompt(record.prompt);
  const skills = parseRunRequestConfigSkills(record.skills);
  const mcpServers = parseRunRequestConfigMcpServers(record.mcpServers);
  return {
    model,
    ...(system !== undefined ? { system } : {}),
    prompt,
    ...(skills !== undefined ? { skills } : {}),
    ...(mcpServers !== undefined ? { mcpServers } : {}),
    // environment / cleanup / proxyEndpoints / metadata: passed through
    // as-is — the BFF revalidates them via `parseRunSubmissionRequest`,
    // so duplicating the heavyweight parsers here would mean two sources
    // of truth. The CLI surfaces structural errors at submission time.
    ...(record.environment !== undefined
      ? { environment: record.environment as NonNullable<RunRequestConfig["environment"]> }
      : {}),
    ...(record.cleanup !== undefined
      ? { cleanup: record.cleanup as NonNullable<RunRequestConfig["cleanup"]> }
      : {}),
    ...(record.runtimeSize !== undefined
      ? { runtimeSize: record.runtimeSize as NonNullable<RunRequestConfig["runtimeSize"]> }
      : {}),
    ...(record.timeout !== undefined
      ? { timeout: record.timeout as NonNullable<RunRequestConfig["timeout"]> }
      : {}),
    ...(record.proxyEndpoints !== undefined
      ? { proxyEndpoints: record.proxyEndpoints as NonNullable<RunRequestConfig["proxyEndpoints"]> }
      : {}),
    ...(record.metadata !== undefined
      ? { metadata: record.metadata as NonNullable<RunRequestConfig["metadata"]> }
      : {})
  };
}

function parseRunRequestConfigPrompt(value: unknown): string | readonly string[] {
  if (typeof value === "string") {
    if (value.length === 0) {
      throw new Error("run request config prompt must be a non-empty string");
    }
    return value;
  }
  if (Array.isArray(value)) {
    const arr: string[] = [];
    for (let i = 0; i < value.length; i++) {
      const item = value[i];
      if (typeof item !== "string" || item.length === 0) {
        throw new Error(`run request config prompt[${i}] must be a non-empty string`);
      }
      arr.push(item);
    }
    if (arr.length === 0) {
      throw new Error("run request config prompt must be a non-empty string or array of strings");
    }
    return arr;
  }
  throw new Error("run request config prompt must be a string or array of strings");
}

function parseRunRequestConfigSkills(value: unknown): readonly SkillRef[] | undefined {
  if (value === undefined) {
    return undefined;
  }
  if (!Array.isArray(value)) {
    throw new Error("run request config skills must be an array");
  }
  return value.map((item, index) =>
    parseSkillRef(item, `run request config skills[${index}]`)
  );
}

function parseRunRequestConfigMcpServers(value: unknown): readonly RunConfigMcpServer[] | undefined {
  if (value === undefined) {
    return undefined;
  }
  if (!Array.isArray(value)) {
    throw new Error("run request config mcpServers must be an array");
  }
  const seen = new Set<string>();
  return value.map((item, index) => {
    const entry = parseRunConfigMcpServerRef(item, `run request config mcpServers[${index}]`);
    if (seen.has(entry.name)) {
      throw new Error(`run request config mcpServers duplicate name: ${entry.name}`);
    }
    seen.add(entry.name);
    return entry;
  });
}

// ---------------------------------------------------------------------------
// Normalisation: run config -> wire-ready submission + secrets split
// ---------------------------------------------------------------------------

/**
 * Result of splitting a run config into the non-secret submission and the
 * secret MCP-headers bundle. The SDK calls this just before posting to
 * /api/runs: the `submission` half is what the BFF hashes for idempotency, the
 * `mcpServerSecrets` half is what enters run-scoped custody.
 *
 * `prompt` is normalised to `readonly string[]` (single-string callers
 * get wrapped in a length-1 array) so the wire payload, the worker, and
 * the audit log don't have to re-handle two shapes.
 */
export interface NormalisedRunRequestConfig {
  readonly model: string;
  readonly system?: string;
  readonly prompt: readonly string[];
  readonly skills: readonly SkillRef[];
  readonly mcpServers: readonly McpServerRef[];
  readonly environment?: PlatformEnvironment;
  readonly cleanup?: PlatformCleanupPolicy;
  readonly proxyEndpoints?: readonly PlatformProxyEndpoint[];
  readonly metadata?: Readonly<Record<string, JsonValue>>;
  /**
   * MCP servers whose run-config entry carried `headers`. Keyed by the `name`
   * that appears in `mcpServers` so the BFF can pair them up.
   */
  readonly mcpServerSecrets: ReadonlyArray<{
    readonly name: string;
    readonly url: string;
    readonly headers: Readonly<Record<string, string>>;
  }>;
}

export function normaliseRunRequestConfig(config: RunRequestConfig): NormalisedRunRequestConfig {
  const prompt: readonly string[] =
    typeof config.prompt === "string" ? [config.prompt] : config.prompt;
  const skills: readonly SkillRef[] = config.skills ?? [];
  const mcpServers: McpServerRef[] = [];
  const mcpServerSecrets: NormalisedRunRequestConfig["mcpServerSecrets"][number][] = [];
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
    skills,
    mcpServers,
    ...(config.environment !== undefined ? { environment: config.environment } : {}),
    ...(config.cleanup !== undefined ? { cleanup: config.cleanup } : {}),
    ...(config.proxyEndpoints !== undefined ? { proxyEndpoints: config.proxyEndpoints } : {}),
    ...(config.metadata !== undefined ? { metadata: config.metadata } : {}),
    mcpServerSecrets
  };
}
