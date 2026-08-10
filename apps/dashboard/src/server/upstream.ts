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
export const CENTRAL_URL_KEY = "AEX_CENTRAL_URL";

type Environment = Readonly<Record<string, string | undefined>>;

export function isRegionCode(value: string): value is RegionCode {
  return (REGION_CODES as readonly string[]).includes(value);
}

export function centralBaseUrl(environment: Environment = process.env): string {
  const configured = environment[CENTRAL_URL_KEY]?.trim();
  if (!configured) {
    throw new Error(`${CENTRAL_URL_KEY} is required; the dashboard never defaults to another plane`);
  }
  const resolved = resolveCentralBaseUrl(configured);
  const url = new URL(resolved);
  if (url.pathname !== "/") {
    throw new Error(`${CENTRAL_URL_KEY} must be a bare HTTPS origin`);
  }
  return url.origin;
}

/** Resolve a regional hostname inside the same configured plane as central. */
export function regionalBaseUrl(
  region: RegionCode,
  environment: Environment = process.env,
): string {
  const central = new URL(centralBaseUrl(environment));
  const productionRegional = new URL(regionalHost(region));
  const suffix = ".api.aex.dev";
  if (!productionRegional.hostname.endsWith(suffix)) {
    throw new Error(`the SDK returned an unsupported regional host for ${region}`);
  }
  const awsRegion = productionRegional.hostname.slice(0, -suffix.length);
  central.hostname = `${awsRegion}.${central.hostname}`;
  return central.origin;
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
  return new FetchTransport({ central, regional: region === null ? central : regionalBaseUrl(region) });
}
