import { createHash } from "node:crypto";

/**
 * Canonical hosted aex API plane URL. Used as the default `baseUrl`
 * for the SDK `Aex` client and the host-side CLI `--aex-url`
 * flag.
 *
 * Pinned to `api.aex.dev` on purpose: the dashboard at
 * `aex.dev` is the human UX surface, while `api.aex.dev`
 * owns the runtime/API plane (run submission, provider proxy, MCP
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
 * Plane → API base URL. When the SDK constructor is given no explicit `baseUrl`
 * it DERIVES the target from the API key's embedded plane (see
 * {@link import("./api-key.js").parseApiKey}) rather than blindly defaulting to
 * prod — so a dev key never silently 401s against `api.aex.dev`.
 *
 * `prd` is {@link AEX_DEFAULT_BASE_URL}. `dev` is `null`: the dev plane has no
 * stable public hostname yet, so a dev key still requires an explicit `baseUrl`
 * — but the plane-mismatch guard (dev key + prd baseUrl) still fires.
 */
export const PLANE_BASE_URLS = {
  dev: null,
  prd: AEX_DEFAULT_BASE_URL
} as const satisfies Readonly<Record<"dev" | "prd", string | null>>;

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
