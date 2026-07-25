/**
 * Single source of truth for a session's artifact namespaces.
 *
 * Every session stores public files and internal diagnostics under its
 * workspace-scoped session prefix:
 *
 *   workspaces/<workspaceId>/sessions/<sessionId>/checkpoints/current.json
 *   workspaces/<workspaceId>/sessions/<sessionId>/checkpoints/<checkpointId>/workspace/index.json
 *   workspaces/<workspaceId>/sessions/<sessionId>/checkpoints/<checkpointId>/workspace/objects/<fileId>
 *   workspaces/<workspaceId>/sessions/<sessionId>/internal/logs/<rel>
 *
 * The latest complete checkpoint is the recovery source and the public files
 * source. Diagnostics reach storage as workspace dotdirs (`.runtime-logs/...`,
 * `.host-logs/...`, etc.); the stored path uses canonical log namespaces.
 */

export const WORKSPACES_PREFIX = "workspaces";
export const SESSIONS_PREFIX = "sessions";
export const SESSION_CHECKPOINTS_PREFIX = "checkpoints";
export const SESSION_INTERNAL_PREFIX = "internal";
export const SESSION_INTERNAL_LOGS_PREFIX = `${SESSION_INTERNAL_PREFIX}/logs`;
export const S3_RETENTION_TAG_KEY = "aex-retention";
export const S3_RETENTION_TAG_VALUES = [
  "session-file-current",
  "workspace-asset",
  "session-core",
  "checkpoint-pending",
  "checkpoint-superseded",
  "upload-staging",
  "internal-log",
  "internal-event",
  "internal-usage",
  "admin-archive"
] as const;

export type S3ObjectRetentionTagValue = (typeof S3_RETENTION_TAG_VALUES)[number];

/**
 * Internal raw-usage export namespace (cost & usage metering). Lives under
 * `internal/` so public downloads never include it by default. Holds
 * `SessionUsageSample` ndjson — the durable system of record for raw usage, written
 * by the coordinator event stream; the public-safe `session_cost` table holds the summary.
 */
export const SESSION_INTERNAL_USAGE_PREFIX = `${SESSION_INTERNAL_PREFIX}/usage`;

/** Object-key prefix (trailing slash) every session-local object for a session lives under. */
export function runSessionPrefix(workspaceId: string, sessionId: string): string {
  return `${WORKSPACES_PREFIX}/${workspaceId}/${SESSIONS_PREFIX}/${sessionId}/`;
}

/** Object key for a session's raw-usage ndjson chunk (n = monotonic chunk index). */
export function runInternalUsageKey(workspaceId: string, sessionId: string, chunk: number): string {
  return `${runSessionPrefix(workspaceId, sessionId)}${SESSION_INTERNAL_USAGE_PREFIX}/${chunk}.ndjson`;
}

/** Object-key prefix (trailing slash) for a session's current-file checkpoints. */
export function runCheckpointPrefix(workspaceId: string, sessionId: string): string {
  return `${runSessionPrefix(workspaceId, sessionId)}${SESSION_CHECKPOINTS_PREFIX}/`;
}

/** Pointer to the latest complete checkpoint. Written last during checkpoint commit. */
export function runCheckpointCurrentKey(workspaceId: string, sessionId: string): string {
  return `${runCheckpointPrefix(workspaceId, sessionId)}current.json`;
}

/** Optional compact list of available checkpoints for admin/debug views. */
export function runCheckpointIndexKey(workspaceId: string, sessionId: string): string {
  return `${runCheckpointPrefix(workspaceId, sessionId)}index.json`;
}

export function runCheckpointManifestKey(workspaceId: string, sessionId: string, checkpointId: string): string {
  return `${runCheckpointPrefix(workspaceId, sessionId)}${checkpointId}/manifest.json`;
}

export function runCheckpointWorkspaceIndexKey(workspaceId: string, sessionId: string, checkpointId: string): string {
  return `${runCheckpointPrefix(workspaceId, sessionId)}${checkpointId}/workspace/index.json`;
}

export function runCheckpointWorkspaceObjectKey(
  workspaceId: string,
  sessionId: string,
  checkpointId: string,
  fileId: string
): string {
  return `${runCheckpointPrefix(workspaceId, sessionId)}${checkpointId}/workspace/objects/${fileId}`;
}

export function runCheckpointBrainJournalKey(workspaceId: string, sessionId: string, checkpointId: string): string {
  return `${runCheckpointPrefix(workspaceId, sessionId)}${checkpointId}/brain/journal.jsonl`;
}

export function runCheckpointBrainStateKey(workspaceId: string, sessionId: string, checkpointId: string): string {
  return `${runCheckpointPrefix(workspaceId, sessionId)}${checkpointId}/brain/state.json`;
}

/** Object-key prefix (trailing slash) for operator-built archive zips for a session. */
export function runAdminArchivePrefix(workspaceId: string, sessionId: string): string {
  return `${runSessionPrefix(workspaceId, sessionId)}${SESSION_INTERNAL_PREFIX}/admin-archives/`;
}

/** Object-key prefix (trailing slash) for generated event archive files for a session. */
export function runEventArchivePrefix(workspaceId: string, sessionId: string): string {
  return `${runSessionPrefix(workspaceId, sessionId)}${SESSION_INTERNAL_PREFIX}/events/`;
}

/**
 * Relative-path prefixes that mark a stored artifact as a platform diagnostic
 * rather than a session deliverable. Dotted forms are upload-time paths.
 */
export const SESSION_LOG_REL_PREFIXES = [
  ".runtime-logs/",
  ".host-logs/",
  ".provider-proxy/",
  ".control-plane/",
  ".anthropic-debug/",
  "runtime/",
  "host/",
  "provider-proxy/",
  "control-plane/",
  "anthropic-debug/"
] as const;

/** True when a workspace-relative artifact path belongs to the internal logs namespace. */
export function isSessionLogRelPath(rel: string): boolean {
  return SESSION_LOG_REL_PREFIXES.some((prefix) => matchesRelPrefix(rel, prefix));
}

/**
 * The artifact's path relative to its namespace prefix as stored. Diagnostics
 * are canonicalized (`.runtime-logs/x` -> `runtime/x`).
 */
export function sessionArtifactRel(rel: string): string {
  if (rel.startsWith(".runtime-logs/")) return `runtime/${rel.slice(".runtime-logs/".length)}`;
  if (rel.startsWith("runtime/")) return rel;
  if (rel.startsWith(".host-logs/")) return `host/${rel.slice(".host-logs/".length)}`;
  if (rel.startsWith("host/")) return rel;
  if (rel.startsWith(".provider-proxy/")) return `provider-proxy/${rel.slice(".provider-proxy/".length)}`;
  if (rel.startsWith("provider-proxy/")) return rel;
  if (rel.startsWith(".control-plane/")) return `control-plane/${rel.slice(".control-plane/".length)}`;
  if (rel.startsWith("control-plane/")) return rel;
  if (rel.startsWith(".anthropic-debug/")) return `provider-proxy/${rel.slice(".anthropic-debug/".length)}`;
  if (rel.startsWith("anthropic-debug/")) return `provider-proxy/${rel.slice("anthropic-debug/".length)}`;
  return rel;
}

/**
 * Storage key for a session artifact uploaded with relative path `rel`, routing
 * diagnostics into `internal/logs/`. Workspace files are stored by checkpoint
 * helpers, not by this function.
 */
export function sessionInternalArtifactKey(workspaceId: string, sessionId: string, rel: string): string {
  return `${runSessionPrefix(workspaceId, sessionId)}${SESSION_INTERNAL_LOGS_PREFIX}/${sessionArtifactRel(rel)}`;
}

/**
 * Opaque, non-reversible file id. Derived as SHA-256 hex of
 * `sessionId:relPath` so a caller cannot construct an id by guessing a filename.
 * It is the single id scheme for session files: the checkpoint index records it,
 * the list route emits it, the download/link routes resolve it back to a path
 * from the latest checkpoint index, and the object key uses it directly. Hex is
 * URL-safe. WebCrypto, no new dep.
 */
export async function opaqueSessionFileId(sessionId: string, relPath: string): Promise<string> {
  const data = new TextEncoder().encode(`${sessionId}:${relPath}`);
  const digest = await crypto.subtle.digest("SHA-256", data);
  return Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, "0")).join("");
}

export interface SessionCheckpointCurrent {
  readonly v: 2;
  readonly workspaceId: string;
  readonly sessionId: string;
  readonly checkpointId: string;
  readonly runId: string;
  readonly turnSeq: number;
  readonly manifestKey: string;
  readonly committedAt: string;
  readonly throughSeq: number;
}

/** Public identity of one complete, immutable session-files checkpoint. */
export interface SessionCheckpointRevision {
  readonly checkpointId: string;
  readonly runId: string;
  readonly turnSeq: number;
  readonly committedAt: string;
  readonly throughSeq: number;
}

export interface SessionCheckpointIndex {
  readonly v: 2;
  readonly workspaceId: string;
  readonly sessionId: string;
  readonly currentCheckpointId: string;
  readonly checkpoints: readonly SessionCheckpointIndexEntry[];
}

export interface SessionCheckpointIndexEntry {
  readonly checkpointId: string;
  readonly runId: string;
  readonly turnSeq: number;
  readonly manifestKey: string;
  readonly committedAt: string;
  readonly throughSeq: number;
  readonly fileCount: number;
  readonly totalBytes: number;
}

export interface SessionCheckpointManifest {
  readonly v: 2;
  readonly workspaceId: string;
  readonly sessionId: string;
  readonly checkpointId: string;
  readonly runId: string;
  readonly turnSeq: number;
  readonly status: "complete";
  readonly createdAt: string;
  readonly committedAt: string;
  readonly throughSeq: number;
  readonly workspace: {
    readonly indexKey: string;
    readonly fileCount: number;
    readonly totalBytes: number;
  };
  readonly brain?: {
    readonly journalKey?: string;
    readonly stateKey?: string;
  };
}

export interface SessionWorkspaceFile {
  readonly fileId: string;
  readonly path: string;
  readonly sizeBytes: number;
  readonly contentType: string;
  readonly objectKey: string;
  readonly sha256: string;
  readonly createdAt: string;
}

export interface SessionCheckpointWorkspaceIndex {
  readonly v: 2;
  readonly workspaceId: string;
  readonly sessionId: string;
  readonly checkpointId: string;
  readonly runId: string;
  readonly turnSeq: number;
  readonly fileCount: number;
  readonly totalBytes: number;
  readonly files: readonly SessionWorkspaceFile[];
}

function matchesRelPrefix(rel: string, prefix: string): boolean {
  if (prefix.endsWith("/")) return rel.startsWith(prefix);
  return rel === prefix || rel.startsWith(`${prefix}/`);
}

/** The `asset_` id form the API emits for a workspace asset. */
export const WORKSPACE_ASSET_ID_PREFIX = "asset_" as const;

/** The `sha256:` content-hash URI form carried on pinned resource versions. */
export const WORKSPACE_ASSET_HASH_URI_PREFIX = "sha256:" as const;

/** A workspace asset is addressed by a bare lowercase SHA-256 hex digest; nothing else. */
const WORKSPACE_ASSET_HASH_HEX = /^[0-9a-f]{64}$/u;

/**
 * Either the canonical key for a valid asset id, or an explicit rejection. There
 * is no third outcome and no permissive variant: an id the API answers 400 for
 * must not silently become a readable object key on the client side.
 */
export type WorkspaceAssetKeyResult =
  | { readonly ok: true; readonly key: string; readonly assetId: string }
  | { readonly ok: false };

/**
 * Storage key for a workspace's snapshotted asset, addressed by its content hash.
 * Used only for pre-checkpoint reusable workspace assets; session boot
 * materializes those assets into an initial checkpoint before execution.
 *
 * Accepts `asset_<hex>`, `sha256:<hex>`, and the bare hex, and rejects everything
 * else. This repo cannot import the platform's owner module, so this is a
 * DELIBERATE MIRROR held byte-identical by a cross-repo parity test
 * (`platform/scripts/validate/object-store-key-public-parity.test.ts`). Change
 * one side and that test fails; do not change one side alone.
 */
export function workspaceAssetKey(workspaceId: string, rawAssetId: string): WorkspaceAssetKeyResult {
  let decoded: string;
  try {
    decoded = decodeURIComponent(rawAssetId).trim();
  } catch {
    // A malformed percent-escape is an invalid asset id, not an internal error.
    return { ok: false };
  }
  const hex = decoded.startsWith(WORKSPACE_ASSET_ID_PREFIX)
    ? decoded.slice(WORKSPACE_ASSET_ID_PREFIX.length)
    : decoded.startsWith(WORKSPACE_ASSET_HASH_URI_PREFIX)
      ? decoded.slice(WORKSPACE_ASSET_HASH_URI_PREFIX.length)
      : decoded;
  if (!WORKSPACE_ASSET_HASH_HEX.test(hex)) return { ok: false };
  return {
    ok: true,
    key: `${WORKSPACES_PREFIX}/${workspaceId}/assets/content/${hex}`,
    assetId: `${WORKSPACE_ASSET_ID_PREFIX}${hex}`
  };
}

/**
 * Retention class for an S3 key. Writers set this as the `aex-retention` object
 * tag so bucket lifecycle rules can apply per logical namespace even though the
 * useful category sits after dynamic workspace/session ids.
 */
export function s3ObjectRetentionTagValue(key: string): S3ObjectRetentionTagValue | undefined {
  const normalized = key.replace(/^\/+/, "");
  if (normalized.startsWith(`${WORKSPACES_PREFIX}/`)) {
    const parts = normalized.split("/");
    if (parts[2] === "assets" && parts[3] === "content" && parts.length >= 5) return "workspace-asset";
    if (parts[2] === "uploads" && parts.length >= 4) return "upload-staging";
    if (parts[2] !== SESSIONS_PREFIX || parts.length < 5) return undefined;
    const rel = parts.slice(4).join("/");
    if (rel.startsWith(`${SESSION_CHECKPOINTS_PREFIX}/`)) {
      if (rel.includes("/workspace/objects/")) return "session-file-current";
      return "session-core";
    }
    if (rel.startsWith(`${SESSION_INTERNAL_PREFIX}/admin-archives/`)) return "admin-archive";
    if (rel.startsWith(`${SESSION_INTERNAL_PREFIX}/events/`)) return "internal-event";
    if (rel.startsWith(`${SESSION_INTERNAL_USAGE_PREFIX}/`)) return "internal-usage";
    if (rel.startsWith(`${SESSION_INTERNAL_LOGS_PREFIX}/`)) return "internal-log";
    if (rel.startsWith(`${SESSION_INTERNAL_PREFIX}/`)) return "internal-log";
    return undefined;
  }

  // Current control-plane core is still read by the running container from the
  // internal control namespace. It is not a public files namespace and can be cut
  // over independently after boot/journal consumers move to workspace-scoped keys.
  if (normalized.startsWith(`${SESSIONS_PREFIX}/`)) {
    const parts = normalized.split("/");
    if (parts.length < 4 || parts[2] !== "session") return undefined;
    const rel = parts.slice(3).join("/");
    if (rel.startsWith("logs/")) return "internal-log";
    if (rel.startsWith("internal/usage/")) return "internal-usage";
    if (rel.startsWith("internal/events/")) return "internal-event";
    if (rel.startsWith("admin-archives/") || rel.startsWith("internal/admin-archives/")) return "admin-archive";
    if (rel === "terminal-failure.json") return "internal-log";
    if (
      rel === "boot.json" ||
      rel === "settle.json" ||
      rel === "control.json" ||
      rel === "secrets.sealed" ||
      rel.startsWith("journal/") ||
      rel.startsWith("turns/")
    ) {
      return "session-core";
    }
  }

  return undefined;
}

/** S3 PutObject/CreateMultipartUpload/Upload `Tagging` header value for a key. */
export function s3ObjectRetentionTaggingHeader(key: string): string | undefined {
  const value = s3ObjectRetentionTagValue(key);
  if (value === undefined) return undefined;
  return `${encodeURIComponent(S3_RETENTION_TAG_KEY)}=${encodeURIComponent(value)}`;
}
