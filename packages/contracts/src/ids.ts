/**
 * The identifier authority for every aex entity.
 *
 * One module declares the format, mints the value, and exports the parser.
 * Every other site — database CHECK constraint, HTTP handler, dashboard URL,
 * log line, span attribute, test fixture — imports from here.
 *
 * An identifier has exactly ONE string form: `<prefix>_<32 lowercase hex>`. It
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
  workspace: "wsp",
  session: "ses",
  resource: "wres",
  mcp: "mcp",
  secret: "sec",
  org: "org",
  team: "team",
  user: "usr",
  apiKey: "key",
  idempotency: "idem"
} as const;

export type IdKind = keyof typeof ID_PREFIXES;
export type IdPrefix = (typeof ID_PREFIXES)[IdKind];

/** `wsp_5fc4b90e55af46cf9938b70f988e431d` — the only form of a workspace id. */
export type Id<K extends IdKind> = `${(typeof ID_PREFIXES)[K]}_${string}`;

export const ID_KINDS: readonly IdKind[] = Object.keys(ID_PREFIXES) as IdKind[];

/**
 * The ONE shape source. Every regex, every SQL CHECK, and every JSON-Schema
 * `pattern` in this workspace is derived from this string — see
 * `idPatternSource` and `platform/scripts/validate/id-format-parity.test.ts`,
 * which fails the build on any hand-written `^<prefix>_` literal elsewhere.
 */
const ID_BODY_PATTERN = "[0-9a-f]{32}";

/** 16 random bytes rendered as 32 lowercase hex characters. */
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

const HEX_DIGITS = "0123456789abcdef";

function toHex(bytes: Uint8Array): string {
  let out = "";
  for (const byte of bytes) {
    out += HEX_DIGITS[byte >> 4]! + HEX_DIGITS[byte & 0x0f]!;
  }
  return out;
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
  return `${prefix}_${toHex(source.getRandomValues(new Uint8Array(ID_BYTES)))}` as Id<K>;
}

/** True when `value` is a well-formed identifier of exactly `kind`. */
export function isId<K extends IdKind>(kind: K, value: unknown): value is Id<K> {
  return typeof value === "string" && idPattern(kind).test(value);
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
