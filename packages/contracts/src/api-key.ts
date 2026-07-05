/**
 * Self-describing API-key codec (SSoT).
 *
 * An aex API key is `aex_<plane>_<regionCode>_<workspaceId>_<secret>_<crc>` —
 * it embeds its own plane and region, CRC-protected. This is the canonical
 * client-side codec: the SDK constructor parses the key to DERIVE the target
 * plane (zero-network) and to fail fast on a plane/baseUrl mismatch instead of
 * surfacing a bare `token_invalid` after a full round-trip.
 *
 * Ported byte-for-byte from the platform codec
 * (`aex-platform apps/dashboard/src/server/auth.ts`), which is pinned to this
 * module by a cross-repo parity test. Parse-only: it makes NO trust decision
 * (the server still validates the secret).
 */

export const API_KEY_PLANES = ["dev", "prd"] as const;
export type ApiKeyPlane = (typeof API_KEY_PLANES)[number];

/** Supported region → embedded region code. */
export const API_KEY_REGION_TO_CODE: Readonly<Record<string, string>> = {
  "eu-west-2": "euw2",
  "us-west-2": "usw2",
  "ap-northeast-1": "apn1"
};

const CODE_TO_REGION: Readonly<Record<string, string>> = Object.fromEntries(
  Object.entries(API_KEY_REGION_TO_CODE).map(([region, code]) => [code, region])
);

const API_KEY_PLANE_SET: ReadonlySet<string> = new Set(API_KEY_PLANES);

export interface ParsedApiKey {
  readonly plane: ApiKeyPlane;
  readonly regionCode: string;
  readonly region: string;
  /** The dash-free workspace id embedded in the key. */
  readonly workspaceId: string;
}

/**
 * Canonical form of a workspaceId for EMBEDDING in a key: dashes stripped so the
 * whole key is a single double-click-selectable word (`_` is a word char, `-`
 * is not). Kept byte-for-byte in sync with the platform codec.
 */
export function normalizeWorkspaceId(workspaceId: string): string {
  return workspaceId.replace(/-/g, "");
}

/**
 * Parse a self-describing API key, or `null` for any legacy/opaque/tampered
 * value. Validates the `aex_` prefix, the 6-part shape, a known plane and
 * region code, and the CRC over the first 5 parts.
 */
export function parseApiKey(token: string): ParsedApiKey | null {
  if (typeof token !== "string" || !token.startsWith("aex_")) return null;
  const parts = token.split("_");
  if (parts.length !== 6) return null;
  const [prefix, plane, regionCode, workspaceId, secret, crc] = parts as [
    string,
    string,
    string,
    string,
    string,
    string
  ];
  if (prefix !== "aex" || !API_KEY_PLANE_SET.has(plane) || !regionCode || !workspaceId || !secret || !crc) {
    return null;
  }
  if (crc32Base36(parts.slice(0, 5).join("_")) !== crc) return null;
  const region = CODE_TO_REGION[regionCode];
  if (region === undefined) return null;
  return { plane: plane as ApiKeyPlane, regionCode, region, workspaceId };
}

/**
 * Assemble a valid API key from its parts (the inverse of {@link parseApiKey}).
 * Unlike the server's `mintApiKeyValue` this takes an EXPLICIT `secret` so it is
 * deterministic — used by codec round-trip / cross-repo parity tests. The
 * embedded workspace id is dash-normalized; `workspaceId` must not contain `_`.
 */
export function formatApiKey(input: {
  readonly plane: ApiKeyPlane;
  readonly region: string;
  readonly workspaceId: string;
  readonly secret: string;
}): string {
  const code = API_KEY_REGION_TO_CODE[input.region];
  if (code === undefined) {
    throw new Error(`API key region is not supported: ${input.region}`);
  }
  if (!input.workspaceId || input.workspaceId.includes("_")) {
    throw new Error("workspaceId must be non-empty and contain no '_'");
  }
  if (!input.secret || input.secret.includes("_")) {
    throw new Error("secret must be non-empty and contain no '_'");
  }
  const body = ["aex", input.plane, code, normalizeWorkspaceId(input.workspaceId), input.secret].join("_");
  return `${body}_${crc32Base36(body)}`;
}

function crc32Base36(input: string): string {
  return crc32(input).toString(36);
}

function crc32(input: string): number {
  let crc = 0xffffffff;
  for (const byte of Buffer.from(input, "utf8")) {
    crc ^= byte;
    for (let i = 0; i < 8; i += 1) {
      crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}
