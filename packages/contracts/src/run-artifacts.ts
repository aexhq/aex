/**
 * Single source of truth for a run's R2 artifact namespaces.
 *
 * Every run stores its artifacts under two sibling prefixes:
 *
 *   runs/<runId>/outputs/<rel>   — the run's real deliverables.
 *   runs/<runId>/logs/<rel>      — platform diagnostics (the
 *                                  `anthropic-debug/`, `goose-logs/`,
 *                                  `fly-logs/` artifacts).
 *
 * The runner uploads every file with a workspace-relative path and the
 * server decides the namespace from that path's prefix, so the split is
 * owned here — the runner does not need to know about it. Diagnostics
 * reach the upload route as workspace dotdirs (`.goose-logs/...`); the
 * leading dot is stripped on the way into the `logs` namespace so the
 * stored path reads cleanly (`logs/goose-logs/...`).
 */

export const RUN_OUTPUTS_PREFIX = "outputs";
export const RUN_LOGS_PREFIX = "logs";

/**
 * Relative-path prefixes that mark a stored artifact as a platform
 * diagnostic (the `logs` namespace) rather than a run deliverable. The
 * dotted form is how the runner uploads them; the dot is dropped in the
 * stored key (see {@link runArtifactRel}).
 */
export const RUN_LOG_REL_PREFIXES = [".anthropic-debug/", ".goose-logs/", ".fly-logs/"] as const;

/** True when a workspace-relative artifact path belongs to the `logs` namespace. */
export function isRunLogRelPath(rel: string): boolean {
  return RUN_LOG_REL_PREFIXES.some((prefix) => rel.startsWith(prefix));
}

/**
 * The artifact's path relative to its namespace prefix as STORED. For a
 * diagnostic the leading dot is dropped (`.goose-logs/x` → `goose-logs/x`)
 * so the `logs` namespace has no dotdirs; deliverables are unchanged. The
 * opaque artifact id is computed over this value, so upload and
 * download/list agree.
 */
export function runArtifactRel(rel: string): string {
  if (isRunLogRelPath(rel) && rel.startsWith(".")) return rel.slice(1);
  return rel;
}

/**
 * R2 key for a run artifact uploaded with relative path `rel`, routing
 * diagnostics into `logs/` (dot-stripped) and everything else into
 * `outputs/`. The upload route and the download/list routes both derive
 * keys from here, so the round-trip stays consistent.
 */
export function runArtifactKey(runId: string, rel: string): string {
  const namespace = isRunLogRelPath(rel) ? RUN_LOGS_PREFIX : RUN_OUTPUTS_PREFIX;
  return `runs/${runId}/${namespace}/${runArtifactRel(rel)}`;
}

/** The `assets` namespace under a run's prefix. */
export const RUN_ASSETS_PREFIX = "assets";

/**
 * R2 key for a run's snapshotted asset, addressed by its content hash.
 * At submit, each referenced shared-store asset (`assets/<ws>/<hash>`) is
 * copied here so the run OWNS its bytes: every runtime reads from this
 * run-scoped key, and `DELETE /api/runs/:id` purges the whole `runs/<id>/`
 * prefix — so deleting a run frees its assets, and deleting a shared-store
 * blob never breaks a run that already snapshotted it. Accepts a
 * `sha256:<hex>` hash or a bare 64-hex digest; the stored suffix is the hex.
 */
export function runAssetKey(runId: string, hash: string): string {
  const hex = hash.startsWith("sha256:") ? hash.slice("sha256:".length) : hash;
  return `runs/${runId}/${RUN_ASSETS_PREFIX}/${hex}`;
}
