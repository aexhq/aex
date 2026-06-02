import { createHash } from "node:crypto";

/**
 * Canonical hosted antpath API plane URL. Used as the default `baseUrl`
 * for the SDK `AntpathClient` and the host-side CLI `--antpath-url`
 * flag.
 *
 * Pinned to `api.antpath.ai` on purpose: the dashboard at
 * `www.antpath.ai` is the human UX surface, while `api.antpath.ai`
 * owns the runtime/API plane (run submission, provider proxy, MCP
 * proxy, runner callbacks). Both the apex `antpath.ai` and `www.` host
 * may issue redirects, and HTTP clients strip the `Authorization`
 * header on cross-origin redirects (WHATWG Fetch §5.5) — hitting them
 * would make authenticated calls land as 401s. Always go direct to
 * the API plane.
 *
 * A single canonical default is not a "footnote mode" — it is the
 * canonical product. Self-hosted deployments override via the
 * explicit `baseUrl` / `--antpath-url` parameter. The value lives in
 * source (no env-var override) so the agent reading the SDK call site
 * can see exactly where the call goes.
 *
 * Kept in source with no env-var override so callers and agents can see the
 * exact default API plane.
 */
export const ANTPATH_DEFAULT_BASE_URL = "https://api.antpath.ai";

export function stableStringify(value: unknown): string {
  return JSON.stringify(sortValue(value));
}

export function sha256(value: unknown): string {
  return createHash("sha256").update(typeof value === "string" ? value : stableStringify(value)).digest("hex");
}

function sortValue(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(sortValue);
  }
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value)
        .filter(([, item]) => item !== undefined)
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([key, item]) => [key, sortValue(item)])
    );
  }
  return value;
}
