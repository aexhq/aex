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
 * (`platform apps/dashboard/src/server/auth.ts`), which is pinned to this
 * module by a cross-repo parity test. Parse-only: it makes NO trust decision
 * (the server still validates the secret).
 */

export const API_KEY_PLANES = ["dev", "prd"] as const;
export type ApiKeyPlane = (typeof API_KEY_PLANES)[number];

/** Supported region → embedded region code. */
export const API_KEY_REGION_TO_CODE: Readonly<Record<string, string>> = {
  "eu-west-1": "euw1"
};

const CODE_TO_REGION: Readonly<Record<string, string>> = Object.fromEntries(
  Object.entries(API_KEY_REGION_TO_CODE).map(([region, code]) => [code, region])
);

const API_KEY_PLANE_SET: ReadonlySet<string> = new Set(API_KEY_PLANES);
const PUBLIC_WORKSPACE_ID_RE = /^wsp_([0-9a-f]{32})$/i;
const DASHLESS_UUID_RE = /^[0-9a-f]{32}$/i;
const UUID_RE = /^([0-9a-f]{8})-([0-9a-f]{4})-([0-9a-f]{4})-([0-9a-f]{4})-([0-9a-f]{12})$/i;

export interface ParsedApiKey {
  readonly plane: ApiKeyPlane;
  readonly regionCode: string;
  readonly region: string;
  /** The dash-free workspace id embedded in the key. */
  readonly workspaceId: string;
}

/**
 * Canonical form of a workspaceId for EMBEDDING in a key. Public workspace ids
 * are `wsp_<uuidhex>`, while storage rows still use dashed UUIDs. Keys embed only
 * the hex field so the token remains underscore-delimited and double-click
 * selectable.
 */
export function normalizeWorkspaceId(workspaceId: string): string {
  const trimmed = workspaceId.trim();
  const publicMatch = PUBLIC_WORKSPACE_ID_RE.exec(trimmed);
  if (publicMatch) return publicMatch[1]!.toLowerCase();
  if (DASHLESS_UUID_RE.test(trimmed)) return trimmed.toLowerCase();
  const uuidMatch = UUID_RE.exec(trimmed);
  return uuidMatch ? uuidMatch.slice(1).join("").toLowerCase() : trimmed.replace(/-/g, "");
}

/**
 * Parse a self-describing API key, or `null` for any opaque or STRUCTURALLY malformed value.
 * Validates the `aex_` prefix, the 6-part shape, and a known plane + region code. The trailing
 * tag is an HMAC keyed by the server pepper (WS6/P4) which the SDK does not hold, so this is the
 * pure routing parse; authenticity is verified server-side. A tampered tag parses (routes) and is
 * rejected at auth.
 */
export function parseApiKey(token: string): ParsedApiKey | null {
  if (typeof token !== "string" || !token.startsWith("aex_")) return null;
  const parts = token.split("_");
  if (parts.length !== 6) return null;
  const [prefix, plane, regionCode, workspaceId, secret, tag] = parts as [
    string,
    string,
    string,
    string,
    string,
    string
  ];
  if (prefix !== "aex" || !API_KEY_PLANE_SET.has(plane) || !regionCode || !workspaceId || !secret || !tag) {
    return null;
  }
  // WS6/P4: the tag is an HMAC keyed by the server pepper, which the SDK does not hold — so this is
  // the PURE routing parse (structure only). Authenticity is enforced server-side by verifyTokenTag
  // before any store touch; a tag-tampered token routes and is then rejected at auth.
  const region = CODE_TO_REGION[regionCode];
  if (region === undefined) return null;
  return { plane: plane as ApiKeyPlane, regionCode, region, workspaceId };
}

/**
 * Assemble a valid API key from its parts (the inverse of {@link parseApiKey}).
 * Unlike the server's `mintApiKeyValue` this takes an EXPLICIT `secret` so it is
 * deterministic — used by codec round-trip / cross-repo parity tests. The
 * embedded workspace id is normalized; public `wsp_...` ids are accepted and
 * embedded as their hex field.
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
  const workspaceField = normalizeWorkspaceId(input.workspaceId);
  if (!workspaceField || workspaceField.includes("_")) {
    throw new Error("workspaceId must be non-empty and contain no '_'");
  }
  if (!input.secret || input.secret.includes("_")) {
    throw new Error("secret must be non-empty and contain no '_'");
  }
  const body = ["aex", input.plane, code, workspaceField, input.secret].join("_");
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
