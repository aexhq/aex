import type { RegionCode } from "@aexhq/sdk";

/**
 * The two spellings of a region.
 *
 * A workspace records its placement as the AWS-style name (`eu-west-1`); a
 * credential and the SDK's routing table use the short code (`euw1`). The
 * passthrough needs the short code, so the mapping lives here once.
 * `test/regions.test.ts` pins every row against `regionalHost()` so the table
 * cannot drift from the SDK's own hosts.
 */
export const REGIONS = [
  { region: "us-east-1", code: "use1" },
  { region: "us-east-2", code: "use2" },
  { region: "us-west-2", code: "usw2" },
  { region: "ap-northeast-1", code: "apne1" },
  { region: "eu-west-1", code: "euw1" },
] as const satisfies readonly { readonly region: string; readonly code: RegionCode }[];

export function regionCodeFor(region: string): RegionCode | null {
  return REGIONS.find((row) => row.region === region)?.code ?? null;
}
