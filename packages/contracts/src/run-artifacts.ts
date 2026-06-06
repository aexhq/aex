/**
 * Single source of truth for a run's artifact namespaces.
 *
 * Every run stores public outputs and internal diagnostics under separate
 * prefixes:
 *
 *   runs/<runId>/outputs/<rel>              — the run's real deliverables.
 *   runs/<runId>/internal/logs/<rel>        — platform diagnostics
 *                                             (`runtime/`, `host/`,
 *                                             `provider-proxy/`,
 *                                             `control-plane/`).
 *
 * The runner uploads every file with a workspace-relative path and the
 * server decides the namespace from that path's prefix, so the split is
 * owned here — the runner does not need to know about it. Diagnostics
 * reach the upload route as workspace dotdirs (`.runtime-logs/...`,
 * `.host-logs/...`, etc.) or runtime-managed home-directory state; the stored
 * path uses canonical log namespaces.
 */

export const RUN_OUTPUTS_PREFIX = "outputs";
export const RUN_INTERNAL_LOGS_PREFIX = "internal/logs";

/**
 * Relative-path prefixes that mark a stored artifact as a platform diagnostic
 * rather than a run deliverable. Dotted forms are upload-time paths; legacy
 * forms are accepted so old records normalize to the canonical namespace.
 */
export const RUN_LOG_REL_PREFIXES = [
  ".runtime-logs/",
  ".host-logs/",
  ".provider-proxy/",
  ".control-plane/",
  ".anthropic-debug/",
  ".goose-logs/",
  ".fly-logs/",
  "runtime/",
  "host/",
  "provider-proxy/",
  "control-plane/",
  "anthropic-debug/",
  "goose-logs/",
  "fly-logs/"
] as const;

/** True when a workspace-relative artifact path belongs to the internal logs namespace. */
export function isRunLogRelPath(rel: string): boolean {
  return RUN_LOG_REL_PREFIXES.some((prefix) => rel.startsWith(prefix));
}

/**
 * The artifact's path relative to its namespace prefix as stored. Diagnostics
 * are canonicalized (`.runtime-logs/x` -> `runtime/x`,
 * legacy `.goose-logs/x` -> `runtime/x`, legacy `.fly-logs/x` -> `host/x`).
 */
export function runArtifactRel(rel: string): string {
  if (rel.startsWith(".runtime-logs/")) return `runtime/${rel.slice(".runtime-logs/".length)}`;
  if (rel.startsWith("runtime/")) return rel;
  if (rel.startsWith(".host-logs/")) return `host/${rel.slice(".host-logs/".length)}`;
  if (rel.startsWith("host/")) return rel;
  if (rel.startsWith(".provider-proxy/")) return `provider-proxy/${rel.slice(".provider-proxy/".length)}`;
  if (rel.startsWith("provider-proxy/")) return rel;
  if (rel.startsWith(".control-plane/")) return `control-plane/${rel.slice(".control-plane/".length)}`;
  if (rel.startsWith("control-plane/")) return rel;
  if (rel.startsWith(".goose-logs/")) return `runtime/${rel.slice(".goose-logs/".length)}`;
  if (rel.startsWith("goose-logs/")) return `runtime/${rel.slice("goose-logs/".length)}`;
  if (rel.startsWith(".fly-logs/")) return `host/${rel.slice(".fly-logs/".length)}`;
  if (rel.startsWith("fly-logs/")) return `host/${rel.slice("fly-logs/".length)}`;
  if (rel.startsWith(".anthropic-debug/")) return `provider-proxy/${rel.slice(".anthropic-debug/".length)}`;
  if (rel.startsWith("anthropic-debug/")) return `provider-proxy/${rel.slice("anthropic-debug/".length)}`;
  return rel;
}

/**
 * Storage key for a run artifact uploaded with relative path `rel`, routing
 * diagnostics into `internal/logs/` and everything else into `outputs/`.
 */
export function runArtifactKey(runId: string, rel: string): string {
  const namespace = isRunLogRelPath(rel) ? RUN_INTERNAL_LOGS_PREFIX : RUN_OUTPUTS_PREFIX;
  return `runs/${runId}/${namespace}/${runArtifactRel(rel)}`;
}

/** The `assets` namespace under a run's prefix. */
export const RUN_ASSETS_PREFIX = "assets";

/**
 * Storage key for a run's snapshotted asset, addressed by its content hash.
 * At submit, each referenced shared-store asset is copied here so the run owns
 * its bytes. Accepts a `sha256:<hex>` hash or a bare 64-hex digest; the stored
 * suffix is the hex.
 */
export function runAssetKey(runId: string, hash: string): string {
  const hex = hash.startsWith("sha256:") ? hash.slice("sha256:".length) : hash;
  return `runs/${runId}/${RUN_ASSETS_PREFIX}/${hex}`;
}
