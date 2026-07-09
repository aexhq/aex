import { createHash } from "node:crypto";

/**
 * Canonical hosted aex API plane URL. Used as the default `baseUrl`
 * for the SDK `Aex` client and the host-side CLI `--aex-url`
 * flag.
 *
 * Pinned to `api.aex.dev` on purpose: the dashboard at
 * `aex.dev` is the human UX surface, while `api.aex.dev`
 * owns the runtime/API plane (session submission, provider proxy, MCP
 * proxy, runner callbacks). Both the apex `aex.dev` and `www.` host
 * may issue redirects, and HTTP clients strip the `Authorization`
 * header on cross-origin redirects (WHATWG Fetch §5.5) — hitting them
 * would make authenticated calls land as 401s. Always go direct to
 * the API plane.
 *
 * A single canonical default is not a "footnote mode" — it is the
 * canonical product. Self-hosted deployments override via the
 * explicit `baseUrl` / `--aex-url` parameter. The value lives in
 * source (no env-var override) so the agent reading the SDK call site
 * can see exactly where the call goes.
 *
 * Kept in source with no env-var override so callers and agents can see the
 * exact default API plane.
 */
export const AEX_DEFAULT_BASE_URL = "https://api.aex.dev";

/**
 * Canonical hosted dev API plane URL. Used when a self-describing dev API key is
 * passed with no explicit `baseUrl`.
 */
export const AEX_DEV_BASE_URL = "https://dev-api.aex.dev";

/**
 * Plane → API base URL. When the SDK constructor is given no explicit `baseUrl`
 * it DERIVES the target from the API key's embedded plane (see
 * {@link import("./api-key.js").parseApiKey}) rather than blindly defaulting to
 * prod — so a dev key routes to `dev-api.aex.dev` and a prd key routes to
 * `api.aex.dev`.
 *
 * The plane-mismatch guard still fires when a key is pointed at the wrong
 * canonical host (for example, a dev key with `baseUrl=https://api.aex.dev`).
 */
export const PLANE_BASE_URLS = {
  dev: AEX_DEV_BASE_URL,
  prd: AEX_DEFAULT_BASE_URL
} as const satisfies Readonly<Record<"dev" | "prd", string>>;

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
