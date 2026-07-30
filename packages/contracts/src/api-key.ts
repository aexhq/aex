/**
 * Canonical parser for workspace API-key values.
 *
 * The value identifies only its region and indexed key row. It contains no
 * plane, workspace id, checksum, verifier tag, or other routing authority.
 */
import { isId, type Id } from "./ids.js";

export const WORKSPACE_API_KEY_REGION_CODES = [
  "use1",
  "use2",
  "usw2",
  "apne1",
  "euw1"
] as const;

export type WorkspaceApiKeyRegionCode =
  (typeof WORKSPACE_API_KEY_REGION_CODES)[number];

export const WORKSPACE_API_KEY_REGIONS: Readonly<
  Record<WorkspaceApiKeyRegionCode, string>
> = {
  use1: "us-east-1",
  use2: "us-east-2",
  usw2: "us-west-2",
  apne1: "ap-northeast-1",
  euw1: "eu-west-1"
};

export const WORKSPACE_API_KEY_FIELD_COUNT = 5;

export const WORKSPACE_API_KEY_PATTERN =
  /^aex_wk_(use1|use2|usw2|apne1|euw1)_([0-9a-hjkmnp-tv-z]{26})_([A-Za-z0-9_-]{42}[AEIMQUYcgkosw048])$/;

export interface ParsedApiKey {
  readonly regionCode: WorkspaceApiKeyRegionCode;
  readonly region: string;
  readonly keyId: Id<"apiKey">;
}

/**
 * Parse a structurally canonical workspace key without making an
 * authentication decision. The server still verifies the 32-byte secret
 * against the stored one-way verifier.
 */
export function parseApiKey(token: string): ParsedApiKey | null {
  if (typeof token !== "string") return null;
  const match = WORKSPACE_API_KEY_PATTERN.exec(token);
  if (match === null) return null;
  const regionCode = match[1] as WorkspaceApiKeyRegionCode;
  const keyId = `key_${match[2]}`;
  if (!isId("apiKey", keyId)) return null;
  return {
    regionCode,
    region: WORKSPACE_API_KEY_REGIONS[regionCode],
    keyId
  };
}

/** Nullable recognizer spelling retained for parser-convention symmetry. */
export function tryParseApiKey(token: string): ParsedApiKey | null {
  return parseApiKey(token);
}

export function isWorkspaceApiKeyValue(value: unknown): value is string {
  return typeof value === "string" && parseApiKey(value) !== null;
}
