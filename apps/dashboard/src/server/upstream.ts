import {
  FetchTransport,
  regionalHost,
  resolveCentralBaseUrl,
  type AexTransport,
  type RegionCode,
} from "@aexhq/sdk";

/**
 * TODO(cross-stream): a dashboard session is not yet resolvable on the regional
 * plane. `central-authz` exposes an account-token resolution but no session
 * equivalent (plan 13 §7 item 3). The regional passthrough presents the browser
 * session credential unchanged; until identity resolves `DashboardSession` for a
 * regional audience every regional panel answers `unauthenticated` and the UI says
 * so. No second credential path is built here, because every workaround is one.
 */

export const REGION_CODES = ["use1", "use2", "usw2", "apne1", "euw1"] as const;

export const CLIENT_HEADER = "aex-dashboard/0.50.0";

export function isRegionCode(value: string): value is RegionCode {
  return (REGION_CODES as readonly string[]).includes(value);
}

export function centralBaseUrl(): string {
  return resolveCentralBaseUrl(process.env["AEX_CENTRAL_URL"]);
}

/**
 * The one place a hostname is decided. The central host comes from configuration
 * and the regional host from a closed five-member enum; neither is ever a value the
 * browser supplied as a URL.
 */
export function transportFor(plane: "central" | "regional", region: RegionCode | null): AexTransport {
  if (plane === "regional" && region === null) {
    throw new Error("a regional operation requires a region");
  }
  const central = centralBaseUrl();
  return new FetchTransport({ central, regional: region === null ? central : regionalHost(region) });
}
