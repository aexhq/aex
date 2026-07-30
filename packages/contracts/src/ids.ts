/**
 * The identifier authority for every aex entity.
 *
 * One module declares the format, mints the value, and exports the parser.
 * Every other site — database CHECK constraint, HTTP handler, dashboard URL,
 * log line, span attribute, test fixture — imports from here.
 *
 * An identifier has exactly ONE string form: a UUIDv7 encoded as
 * `<prefix>_<26 lowercase Crockford base32>`. It
 * is the same bytes in Aurora, in the API response, in the dashboard URL, in
 * the log line, and in the telemetry attribute. There is deliberately no
 * normalizer and no coercer: a value that must be reshaped to be compared has
 * already drifted, so `assertId` throws instead.
 *
 * This module is isomorphic — `crypto.getRandomValues` only, no `node:crypto`,
 * no `Buffer` — because `@aexhq/contracts` ships to browser and edge runtimes and
 * declares no runtime dependency beyond `fflate`. The token pepper and its HMAC
 * stay private in `@aexhq/contract-core`; nothing secret lives here.
 */

/**
 * Every identifier kind and its wire prefix. A new entity kind is a new entry
 * here, never a new generator.
 */
export const ID_PREFIXES = {
  user: "usr",
  organization: "org",
  membership: "mem",
  invitation: "inv",
  workspace: "wsp",
  apiKey: "key",
  session: "ses",
  message: "msg",
  run: "run",
  agent: "agt",
  toolCall: "tcl",
  operation: "op",
  approval: "apr",
  generation: "gen",
  observation: "obs",
  telemetryBatch: "bch",
  telemetryGap: "gap",
  export: "exp",
  upload: "upl",
  measurement: "msr",
  statement: "stm"
} as const;

export type IdKind = keyof typeof ID_PREFIXES;
export type IdPrefix = (typeof ID_PREFIXES)[IdKind];

/** `wsp_01k1e7x9m8e009x13t2nby2zy9` — the only form of a workspace id. */
export type Id<K extends IdKind> = `${(typeof ID_PREFIXES)[K]}_${string}`;

export const ID_KINDS: readonly IdKind[] = Object.keys(ID_PREFIXES) as IdKind[];

/**
 * The ONE shape source. Every regex, every SQL CHECK, and every JSON-Schema
 * `pattern` in this workspace is derived from this string — see
 * `idPatternSource` and `platform/scripts/validate/id-format-parity.test.ts`,
 * which fails the build on any hand-written `^<prefix>_` literal elsewhere.
 */
const ID_BODY_PATTERN = "[0-9a-hjkmnp-tv-z]{26}";

/** One UUID is 16 bytes before its Crockford-base32 encoding. */
const ID_BYTES = 16;

const PATTERN_CACHE = new Map<IdKind, RegExp>();

/** The regex source for one kind, e.g. `^wsp_[0-9a-f]{32}$`. */
export function idPatternSource(kind: IdKind): string {
  return `^${ID_PREFIXES[kind]}_${ID_BODY_PATTERN}$`;
}

/** The compiled anchored pattern for one kind. Case-sensitive by design. */
export function idPattern(kind: IdKind): RegExp {
  const cached = PATTERN_CACHE.get(kind);
  if (cached) return cached;
  const compiled = new RegExp(idPatternSource(kind));
  PATTERN_CACHE.set(kind, compiled);
  return compiled;
}

const CROCKFORD = "0123456789abcdefghjkmnpqrstvwxyz";

function toBase32(bytes: Uint8Array): string {
  let value = 0n;
  for (const byte of bytes) {
    value = (value << 8n) | BigInt(byte);
  }
  let out = "";
  for (let index = 0; index < 26; index += 1) {
    out = CROCKFORD[Number(value & 31n)]! + out;
    value >>= 5n;
  }
  return out;
}

function isUuidV7Body(value: string): boolean {
  let encoded = 0n;
  for (const character of value) {
    const digit = CROCKFORD.indexOf(character);
    if (digit < 0) return false;
    encoded = (encoded << 5n) | BigInt(digit);
  }
  if (encoded >= (1n << 128n)) return false;
  const bytes = new Uint8Array(ID_BYTES);
  for (let index = ID_BYTES - 1; index >= 0; index -= 1) {
    bytes[index] = Number(encoded & 255n);
    encoded >>= 8n;
  }
  return (bytes[6]! & 0xf0) === 0x70 && (bytes[8]! & 0xc0) === 0x80;
}

/**
 * Mint a fresh identifier of `kind`.
 *
 * Fails loudly when no CSPRNG is reachable rather than degrading to
 * `Math.random`: a guessable workspace or session id is a tenancy boundary
 * failure, not a portability inconvenience.
 */
export function newId<K extends IdKind>(kind: K): Id<K> {
  const prefix = ID_PREFIXES[kind];
  if (prefix === undefined) {
    throw new Error(`newId: unknown id kind "${String(kind)}" (add it to ID_PREFIXES)`);
  }
  const source = globalThis.crypto;
  if (typeof source?.getRandomValues !== "function") {
    throw new Error("newId: crypto.getRandomValues is unavailable; cannot mint an identifier");
  }
  const bytes = source.getRandomValues(new Uint8Array(ID_BYTES));
  const timestamp = Date.now();
  bytes[0] = Math.floor(timestamp / 2 ** 40) & 0xff;
  bytes[1] = Math.floor(timestamp / 2 ** 32) & 0xff;
  bytes[2] = Math.floor(timestamp / 2 ** 24) & 0xff;
  bytes[3] = Math.floor(timestamp / 2 ** 16) & 0xff;
  bytes[4] = Math.floor(timestamp / 2 ** 8) & 0xff;
  bytes[5] = timestamp & 0xff;
  bytes[6] = (bytes[6]! & 0x0f) | 0x70;
  bytes[8] = (bytes[8]! & 0x3f) | 0x80;
  return `${prefix}_${toBase32(bytes)}` as Id<K>;
}

/** True when `value` is a well-formed identifier of exactly `kind`. */
export function isId<K extends IdKind>(kind: K, value: unknown): value is Id<K> {
  if (typeof value !== "string" || !idPattern(kind).test(value)) return false;
  return isUuidV7Body(value.slice(value.indexOf("_") + 1));
}

/**
 * Narrow `value` to an identifier of `kind`, or throw.
 *
 * The message names the kind, the expected shape, and the value it got, because
 * the caller's next action is to find whichever layer produced the wrong form.
 */
export function assertId<K extends IdKind>(kind: K, value: unknown, label?: string): Id<K> {
  if (isId(kind, value)) return value;
  const what = label ?? `${kind} id`;
  const got = typeof value === "string" ? JSON.stringify(value) : `<${typeof value}>`;
  throw new Error(`${what} must match ${idPatternSource(kind)}, got ${got}`);
}

/**
 * The kind `value` belongs to, or `undefined`. Used by generic surfaces (audit
 * rows, error messages) that accept several kinds; a call site that knows the
 * kind must use {@link assertId} instead.
 */
export function idKindOf(value: unknown): IdKind | undefined {
  if (typeof value !== "string") return undefined;
  const underscore = value.indexOf("_");
  if (underscore <= 0) return undefined;
  const prefix = value.slice(0, underscore);
  for (const kind of ID_KINDS) {
    if (ID_PREFIXES[kind] === prefix) return isId(kind, value) ? kind : undefined;
  }
  return undefined;
}
