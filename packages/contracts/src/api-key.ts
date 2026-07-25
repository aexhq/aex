/**
 * Self-describing API-key codec (SSoT).
 *
 * An aex API key is `aex_<plane>_<regionCode>_<workspaceId>_<secret>_<crc>` —
 * it embeds its own plane and region, CRC-protected. This is the canonical
 * client-side codec: the SDK constructor parses the key to DERIVE the target
 * plane (zero-network) and to fail fast on a plane/baseUrl mismatch instead of
 * surfacing a bare `token_invalid` after a full round-trip.
 *
 * The embedded workspace id is a whole `wsp_<32hex>` id ({@link ID_PREFIXES}),
 * which makes the key SEVEN underscore-separated fields rather than six. The old
 * six-field layout stripped the `wsp_` prefix to fit the workspace id into one
 * field; that strip existed because of this wire format, not because of the
 * database, and it is what let four workspace-id string forms coexist.
 *
 * Pinned to the platform codec (`@aexhq/contract-core/token-codec`) by a
 * cross-repo parity test. Parse-only: it makes NO trust decision (the server
 * still validates the secret).
 */

import { assertId, isId } from "./ids.js";

export const API_KEY_PLANES = ["dev", "prd"] as const;
export type ApiKeyPlane = (typeof API_KEY_PLANES)[number];

/**
 * Supported region → embedded region code. Mirrors
 * `@aexhq/contract-core/src/supported-regions.ts` (`REGION_TOKEN_CODES`), which
 * is exhaustive over the launch region set by `satisfies`; the two are pinned to
 * each other by `platform/scripts/validate/api-key-codec-parity.test.ts`. These
 * codes are FROZEN — a code is a permanent field of every key minted with it.
 */
export const API_KEY_REGION_TO_CODE: Readonly<Record<string, string>> = {
  "eu-west-1": "euw1",
  "us-west-1": "usw1",
  "ap-northeast-1": "apne1"
};

const CODE_TO_REGION: Readonly<Record<string, string>> = Object.fromEntries(
  Object.entries(API_KEY_REGION_TO_CODE).map(([region, code]) => [code, region])
);

const API_KEY_PLANE_SET: ReadonlySet<string> = new Set(API_KEY_PLANES);

/** `aex_<plane>_<regionCode>_<wsp>_<hex>_<secret>_<tag>`. */
export const API_KEY_FIELD_COUNT = 7;

export interface ParsedApiKey {
  readonly plane: ApiKeyPlane;
  readonly regionCode: string;
  readonly region: string;
  /** The workspace id embedded in the key, in its one canonical `wsp_<32hex>` form. */
  readonly workspaceId: string;
}

/**
 * Parse a self-describing API key, or `null` for any opaque or STRUCTURALLY malformed value.
 * Validates the `aex_` prefix, the 7-part shape, a known plane + region code, and that the
 * embedded workspace id is a canonical id. The trailing tag is an HMAC keyed by the server
 * pepper (WS6/P4) which the SDK does not hold, so this is the pure routing parse; authenticity
 * is verified server-side. A tampered tag parses (routes) and is rejected at auth.
 */
export function tryParseApiKey(token: string): ParsedApiKey | null {
  if (typeof token !== "string" || !token.startsWith("aex_")) return null;
  const parts = token.split("_");
  if (parts.length !== API_KEY_FIELD_COUNT) return null;
  const [prefix, plane, regionCode, workspacePrefix, workspaceBody, secret, tag] = parts as [
    string,
    string,
    string,
    string,
    string,
    string,
    string
  ];
  if (prefix !== "aex" || !API_KEY_PLANE_SET.has(plane) || !regionCode || !secret || !tag) {
    return null;
  }
  const workspaceId = `${workspacePrefix}_${workspaceBody}`;
  if (!isId("workspace", workspaceId)) return null;
  // WS6/P4: the tag is an HMAC keyed by the server pepper, which the SDK does not hold — so this is
  // the PURE routing parse (structure only). Authenticity is enforced server-side by verifyTokenTag
  // before any store touch; a tag-tampered token routes and is then rejected at auth.
  const region = CODE_TO_REGION[regionCode];
  if (region === undefined) return null;
  return { plane: plane as ApiKeyPlane, regionCode, region, workspaceId };
}

/** @deprecated Use {@link tryParseApiKey}; this compatibility wrapper is identical. */
export function parseApiKey(token: string): ParsedApiKey | null {
  return tryParseApiKey(token);
}

/**
 * Assemble a valid API key from its parts (the inverse of {@link parseApiKey}).
 * Unlike the server's `mintApiKeyValue` this takes an EXPLICIT `secret` so it is
 * deterministic — used by codec round-trip / cross-repo parity tests. The
 * workspace id must ALREADY be canonical: there is no normalizer, so a dashed
 * uuid or a bare hex string throws instead of being silently reshaped.
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
  const workspaceField = assertId("workspace", input.workspaceId, "API key workspaceId");
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
