import { Transform, type TransformCallback } from "node:stream";

export type PublicSafeStructuralReason = "forbidden_field_name" | "uninspectable_value";

export type PublicSafeSecretReason =
  | "bearer_token"
  | "provider_key"
  | "signed_url"
  | "object_store_key"
  | "vault_id"
  | "private_resource_handle"
  | "high_entropy_token";

export interface PublicSafeFinding<Reason extends string> {
  readonly path: string;
  readonly reason: Reason | PublicSafeStructuralReason;
  readonly valueLength?: number;
}

export interface PublicSafeStringPattern<Reason extends string> {
  readonly reason: Reason;
  readonly regex: RegExp;
  readonly accept?: (match: string) => boolean;
}

export interface PublicSafePatternMatch<Reason extends string> {
  readonly reason: Reason;
  readonly path: string;
  readonly value: string;
  readonly match: string;
}

export interface PublicSafeScannerConfig<Reason extends string> {
  readonly patterns: readonly PublicSafeStringPattern<Reason>[];
  readonly isForbiddenFieldName: (key: string, path: string) => boolean;
  readonly isPatternExempt?: (match: PublicSafePatternMatch<Reason>) => boolean;
  readonly maxDepth?: number;
  readonly maxVisitedValues?: number;
}

const DEFAULT_MAX_DEPTH = 64;
const DEFAULT_MAX_VISITED_VALUES = 100_000;
const ENTROPY_BITS_PER_CHAR = 3.0;
const MIN_CHAR_CLASSES = 2;
const HIGH_ENTROPY_NO_DIGIT_MIN_LEN = 40;

/** The only bare content identity the platform mints. */
export function isPlatformContentDigest(candidate: string): boolean {
  return /^[0-9a-f]{64}$/.test(candidate);
}

export function looksHighEntropySecret(value: string): boolean {
  return value.split("-").some(isOpaqueSecretRun);
}

export function publicSafeCharClassCount(value: string): number {
  let count = 0;
  if (/[a-z]/.test(value)) count++;
  if (/[A-Z]/.test(value)) count++;
  if (/[0-9]/.test(value)) count++;
  return count;
}

export function publicSafeShannonBits(value: string): number {
  if (value.length === 0) return 0;
  const counts = new Map<string, number>();
  for (const char of value) {
    counts.set(char, (counts.get(char) ?? 0) + 1);
  }
  let bits = 0;
  for (const count of counts.values()) {
    const probability = count / value.length;
    bits -= probability * Math.log2(probability);
  }
  return bits;
}

function isOpaqueSecretRun(value: string): boolean {
  if (publicSafeCharClassCount(value) < MIN_CHAR_CLASSES) return false;
  if (publicSafeShannonBits(value) < ENTROPY_BITS_PER_CHAR) return false;
  return /[0-9]/.test(value) || value.length >= HIGH_ENTROPY_NO_DIGIT_MIN_LEN;
}

export const PUBLIC_SAFE_SECRET_PATTERNS: readonly PublicSafeStringPattern<
  Exclude<PublicSafeSecretReason, "private_resource_handle">
>[] = Object.freeze([
  Object.freeze({
    reason: "bearer_token" as const,
    regex: /\b(Bearer)\s+[A-Za-z0-9._~+/=-]{8,}/i
  }),
  Object.freeze({
    reason: "provider_key" as const,
    regex: /\b(?:sk-[A-Za-z0-9_-]{16,}|xox[baprs]-[A-Za-z0-9-]{8,}|AIza[A-Za-z0-9_-]{8,})/i
  }),
  Object.freeze({
    reason: "signed_url" as const,
    regex: /\bhttps?:\/\/[^\s"'<>]*(?:X-Amz-Signature|X-Amz-Credential|X-Amz-Algorithm|AWSAccessKeyId|X-Goog-Signature|X-Goog-Credential|[?&]sig=)[^\s"'<>]*/i
  }),
  Object.freeze({
    reason: "object_store_key" as const,
    regex: /(^|[\s"'`])(?:sessions|assets)\/[^?<#\s"'`]+/i
  }),
  Object.freeze({
    reason: "vault_id" as const,
    regex: /\b(?:vault|vlt|secret)[_:-][A-Za-z0-9][A-Za-z0-9_-]{7,}\b/i
  }),
  Object.freeze({
    reason: "high_entropy_token" as const,
    regex: /\b[A-Za-z0-9-]{40,}\b/,
    accept: looksHighEntropySecret
  })
]);

/** Extra entropy policy for consumers whose identifier slots admit `_`. */
export function underscoredHighEntropyPattern(): PublicSafeStringPattern<"high_entropy_token"> {
  return Object.freeze({
    reason: "high_entropy_token",
    regex: /\b[A-Za-z0-9-]{3,}_[A-Za-z0-9_-]{36,}\b/,
    accept: (match: string) => looksHighEntropySecret(match.replaceAll("_", "-"))
  });
}

export function privateResourceHandlePattern(
  keywords: readonly string[],
  separators: "_:-" | "_:" = "_:-"
): PublicSafeStringPattern<"private_resource_handle"> {
  const source = keywords.map(escapePublicSafeRegex).join("|");
  return Object.freeze({
    reason: "private_resource_handle",
    regex: new RegExp(
      `\\b(?:${source})[${separators}][A-Za-z0-9][A-Za-z0-9_-]{7,}\\b`,
      "i"
    ),
    accept: isMintedResourceHandle
  });
}

function isMintedResourceHandle(match: string): boolean {
  const separatorIndex = match.search(/[_:-]/);
  return separatorIndex >= 0 && /\d/.test(match.slice(separatorIndex + 1));
}

function escapePublicSafeRegex(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

export function createForbiddenFieldNamePredicate(
  names: readonly string[]
): (key: string, path: string) => boolean {
  const normalized = new Set(names.map((name) => name.toLowerCase()));
  return (key: string): boolean => normalized.has(key.toLowerCase());
}

export function scanPublicSafePayload<Reason extends string>(
  input: unknown,
  config: PublicSafeScannerConfig<Reason>
): readonly PublicSafeFinding<Reason>[] {
  const findings: PublicSafeFinding<Reason>[] = [];
  const seen = new WeakSet<object>();
  const stack: Array<{ readonly value: unknown; readonly path: string; readonly depth: number }> = [
    { value: input, path: "$", depth: 0 }
  ];
  const maxDepth = config.maxDepth ?? DEFAULT_MAX_DEPTH;
  const maxVisitedValues = config.maxVisitedValues ?? DEFAULT_MAX_VISITED_VALUES;
  let visited = 0;

  while (stack.length > 0) {
    const current = stack.pop()!;
    visited++;
    if (visited > maxVisitedValues || current.depth > maxDepth) {
      findings.push(freezePublicSafeFinding<Reason>({ path: current.path, reason: "uninspectable_value" }));
      continue;
    }
    if (typeof current.value === "string") {
      scanPublicSafeString(current.value, current.path, config, findings);
      continue;
    }
    if (current.value === null || typeof current.value !== "object") continue;
    if (seen.has(current.value)) {
      findings.push(freezePublicSafeFinding<Reason>({ path: current.path, reason: "uninspectable_value" }));
      continue;
    }
    seen.add(current.value);

    let keys: string[];
    try {
      keys = Object.keys(current.value);
    } catch {
      findings.push(freezePublicSafeFinding<Reason>({ path: current.path, reason: "uninspectable_value" }));
      continue;
    }

    const children: Array<{ readonly value: unknown; readonly path: string; readonly depth: number }> = [];
    for (const key of keys) {
      const childPath = Array.isArray(current.value) && /^\d+$/.test(key)
        ? `${current.path}[${key}]`
        : `${current.path}.${key}`;
      let forbidden = false;
      try {
        forbidden = config.isForbiddenFieldName(key, childPath);
      } catch {
        findings.push(freezePublicSafeFinding<Reason>({ path: childPath, reason: "uninspectable_value" }));
      }
      if (forbidden) {
        findings.push(freezePublicSafeFinding<Reason>({ path: childPath, reason: "forbidden_field_name" }));
      }

      let descriptor: PropertyDescriptor | undefined;
      try {
        descriptor = Object.getOwnPropertyDescriptor(current.value, key);
      } catch {
        findings.push(freezePublicSafeFinding<Reason>({ path: childPath, reason: "uninspectable_value" }));
        continue;
      }
      if (!descriptor || !("value" in descriptor)) {
        findings.push(freezePublicSafeFinding<Reason>({ path: childPath, reason: "uninspectable_value" }));
        continue;
      }
      children.push({ value: descriptor.value, path: childPath, depth: current.depth + 1 });
    }
    for (let index = children.length - 1; index >= 0; index--) stack.push(children[index]!);
  }

  return Object.freeze(findings);
}

function scanPublicSafeString<Reason extends string>(
  value: string,
  path: string,
  config: PublicSafeScannerConfig<Reason>,
  findings: PublicSafeFinding<Reason>[]
): void {
  for (const pattern of config.patterns) {
    let matches: readonly string[];
    try {
      matches = matchingPublicSafeSequences(pattern, value);
    } catch {
      findings.push(freezePublicSafeFinding<Reason>({ path, reason: "uninspectable_value" }));
      continue;
    }
    for (const match of matches) {
      let exempt = false;
      try {
        exempt = config.isPatternExempt?.({ reason: pattern.reason, path, value, match }) ?? false;
      } catch {
        findings.push(freezePublicSafeFinding<Reason>({ path, reason: "uninspectable_value" }));
        break;
      }
      if (!exempt) {
        findings.push(freezePublicSafeFinding<Reason>({ path, reason: pattern.reason, valueLength: value.length }));
        break;
      }
    }
  }
}

function matchingPublicSafeSequences<Reason extends string>(
  pattern: PublicSafeStringPattern<Reason>,
  value: string
): readonly string[] {
  const flags = pattern.regex.flags.includes("g") ? pattern.regex.flags : `${pattern.regex.flags}g`;
  const regex = new RegExp(pattern.regex.source, flags);
  const matches: string[] = [];
  let match: RegExpExecArray | null;
  while ((match = regex.exec(value)) !== null) {
    if (!pattern.accept || pattern.accept(match[0])) matches.push(match[0]);
    if (match.index === regex.lastIndex) regex.lastIndex++;
  }
  return matches;
}

function freezePublicSafeFinding<Reason extends string>(
  finding: PublicSafeFinding<Reason>
): PublicSafeFinding<Reason> {
  return Object.freeze(finding);
}

/**
 * Value-AGNOSTIC secret patterns: each matches a SHAPE, not a known value.
 * Correctness must not depend on seeding the redactor with the literal
 * secret — the two real leaks this project hit were a database password that
 * survived a naive `sed` mask (as a substring) and an Anthropic key surfaced in
 * a deploy CLI's describe output that the harness NEVER loaded. A value-seeded
 * redactor is blind to both; these patterns catch them by form.
 *
 * Ordering matters: structured shapes (connection strings, auth headers, JWT)
 * come BEFORE the generic key/entropy catch-alls so the more descriptive
 * `[REDACTED …]` label wins on overlapping matches.
 */
const SECRET_PATTERNS: readonly RegExp[] = [
  // postgres / postgresql connection strings — redact the whole URI so the
  // embedded password can never survive as a substring (the `sed`-mask leak).
  /\bpostgres(?:ql)?:\/\/[^\s"']+/gi,
  // aex workspace / proxy tokens: apt_… / ant_….
  /\b(?:apt|ant)_[A-Za-z0-9_-]{16,}/g,
  // aex self-describing workspace API key (one-time reveal from createWorkspace /
  // createApiKey): aex_<plane>_<region>_<workspaceId>_<secret>_<crc>. Anchored on
  // the full 6-part shape so the whole key masks as one label (the entropy
  // catch-all would otherwise redact only its dense segments, leaving the
  // `aex_<plane>_<region>_` prefix behind). Never matches `api.aex.dev`.
  /\baex_(?:dev|prd)_[a-z0-9]+_[a-z0-9]+_[A-Za-z0-9]+_[a-z0-9]+/gi,
  // aex account PAT (control-plane): aexu_<opaque>. Distinct prefix from the
  // workspace-key family so a PAT masks by shape even when short.
  /\baexu_[A-Za-z0-9_-]{16,}/g,
  // AWS access key id + secret access key shapes.
  /\b(?:AKIA|ASIA)[A-Z0-9]{16}\b/g,
  /\baws_secret_access_key["'\s:=]+[A-Za-z0-9/+=]{40}/gi,
  // JWT-shaped: header.payload.signature, each base64url, header starts `eyJ`.
  /\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}/g,
  // Keyword-introduced secret assignments (bearer/token/api_key/…).
  /(?:bearer|token|api[_-]?key|access[_-]?token|refresh[_-]?token|client[_-]?secret)["'\s:=]+[A-Za-z0-9_\-./+=]{12,}/gi
];

const SDK_SHARED_SECRET_PATTERNS = PUBLIC_SAFE_SECRET_PATTERNS.filter((pattern) =>
  pattern.reason === "bearer_token" ||
  pattern.reason === "provider_key" ||
  pattern.reason === "signed_url"
);

const REDACTED = "[REDACTED]";

/**
 * Generic high-entropy token catch-all, applied AFTER the named patterns.
 * Catches the secret nobody recognised: any long base64url/hex-ish run that is
 * BOTH high-entropy AND character-class-diverse.
 *
 * The candidate run deliberately EXCLUDES `_`: secrets are contiguous opaque
 * runs, whereas `SCREAMING_SNAKE_CASE` env names and `snake_case` identifiers
 * are `_`-separated words — excluding `_` breaks those into sub-24 fragments so
 * they are never eaten. The
 * class-diversity gate (≥2 of lower/upper/digit) then keeps the remaining
 * single-class runs (all-lowercase module paths like `internal/modules/esm`)
 * while still catching mixed-case/alnum secret blobs. Entropy alone cannot
 * separate these — `the_quick…` (4.16) out-scores a real key prefix.
 */
const HIGH_ENTROPY_CANDIDATE = /[A-Za-z0-9+/=-]{24,}/g;
/**
 * A mixed-case run with no digit and < this length is treated as a benign
 * identifier, not a secret. Real opaque secrets are alnum-mixed (carry a
 * digit) or very long; digit-free camelCase identifiers like
 * `asyncEntryPointWithESMLoader` / `getReadableStreamController` (which
 * appear in stack traces the diagnostic bundle captures) are 24–39 chars and
 * digit-free — eating them would gut debuggability. The length escape hatch
 * still catches the rare long digit-free secret.
 */

/**
 * Canonical aex session-id hex: exactly 32 lowercase hex chars directly preceded
 * by `ses_`. The HIGH_ENTROPY_CANDIDATE class excludes `_`, so the candidate
 * candidate for a session id is the bare hex — dense enough to trip the entropy gate.
 * Masking it destroys the ONE identifier every error message needs for
 * traceability (`cancel via aex.sessions.open("ses_[REDACTED]")` is useless
 * guidance). A session id is not a credential: it grants nothing without the
 * bearer token. Mirrors the platform-side redactor's canonical-id exemption.
 */
function isCanonicalSessionIdHex(input: string, matchStart: number, match: string): boolean {
  if (!/^[0-9a-f]{32}$/.test(match)) return false;
  return input.slice(Math.max(0, matchStart - 4), matchStart) === "ses_";
}

export class SecretString {
  readonly #value: string;

  constructor(value: string, label = "secret") {
    if (!value) {
      throw new Error(`${label} is required`);
    }
    this.#value = value;
  }

  unwrap(): string {
    return this.#value;
  }

  toString(): string {
    return REDACTED;
  }

  toJSON(): string {
    return REDACTED;
  }
}

export function redactSecrets<T>(value: T): T {
  if (typeof value === "string") {
    return redactString(value) as T;
  }
  if (Array.isArray(value)) {
    return value.map((item) => redactSecrets(item)) as T;
  }
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [key, item] of Object.entries(value)) {
      if (isSecretKey(key)) {
        out[key] = REDACTED;
      } else {
        out[key] = redactSecrets(item);
      }
    }
    return out as T;
  }
  return value;
}

/**
 * Redact every secret-shaped run in `input`.
 *
 * `known` is an OPTIONAL belt-and-suspenders set of literal values to also mask
 * (e.g. creds the caller happens to have loaded). Correctness does NOT depend on
 * it — every shape above is caught with or without seeding — but masking known
 * values first removes any residual substring of a value the shapes split.
 */
export function redactString(input: string, known: Iterable<string> = []): string {
  let out = input;
  for (const value of known) {
    if (value && value.length >= 4) {
      out = out.split(value).join(REDACTED);
    }
  }
  out = SDK_SHARED_SECRET_PATTERNS.reduce(
    (current, pattern) =>
      current.replace(
        new RegExp(pattern.regex.source, pattern.regex.flags.includes("g")
          ? pattern.regex.flags
          : `${pattern.regex.flags}g`),
        (match, captured?: string) =>
          typeof captured === "string" ? `${captured} ${REDACTED}` : REDACTED
      ),
    out
  );
  out = SECRET_PATTERNS.reduce(
    (current, pattern) =>
      current.replace(pattern, (match, captured?: string) =>
        typeof captured === "string" ? `${captured} ${REDACTED}` : REDACTED
      ),
    out
  );
  return out.replace(HIGH_ENTROPY_CANDIDATE, (match, offset: number, whole: string) =>
    !isCanonicalSessionIdHex(whole, offset, match) &&
    !isPlatformContentDigest(match) &&
    looksHighEntropySecret(match)
      ? REDACTED
      : match
  );
}

export function containsSecretLikeValue(input: string): boolean {
  if (
    [...SECRET_PATTERNS, ...SDK_SHARED_SECRET_PATTERNS.map((pattern) => pattern.regex)].some((pattern) => {
      pattern.lastIndex = 0;
      return pattern.test(input);
    })
  ) {
    return true;
  }
  HIGH_ENTROPY_CANDIDATE.lastIndex = 0;
  let candidate: RegExpExecArray | null;
  while ((candidate = HIGH_ENTROPY_CANDIDATE.exec(input)) !== null) {
    if (isCanonicalSessionIdHex(input, candidate.index, candidate[0])) {
      continue;
    }
    if (isPlatformContentDigest(candidate[0])) {
      continue;
    }
    if (looksHighEntropySecret(candidate[0])) {
      return true;
    }
  }
  return false;
}

/**
 * A line-buffered `Transform` that redacts secrets BEFORE bytes touch disk or
 * console. Pipe a child process's stdout/stderr through it
 * (`child.stdout.pipe(createRedactingStream()).pipe(process.stdout)`) so a
 * secret can never be written and "cleaned up after"; the secret you didn't
 * recognise is the one already on disk.
 *
 * Buffers by line so a secret split across a chunk boundary is still redacted:
 * a partial trailing line is held until the next chunk (or `flush`) completes
 * it. `known` is forwarded to `redactString` for optional value seeding.
 */
export function createRedactingStream(known: Iterable<string> = []): Transform {
  const knownValues = [...known];
  let carry = "";
  return new Transform({
    transform(chunk: Buffer | string, _encoding, callback: TransformCallback) {
      carry += chunk.toString();
      const lastNewline = carry.lastIndexOf("\n");
      if (lastNewline === -1) {
        callback();
        return;
      }
      const ready = carry.slice(0, lastNewline + 1);
      carry = carry.slice(lastNewline + 1);
      callback(null, redactString(ready, knownValues));
    },
    flush(callback: TransformCallback) {
      if (carry === "") {
        callback();
        return;
      }
      const tail = carry;
      carry = "";
      callback(null, redactString(tail, knownValues));
    }
  });
}

function isSecretKey(key: string): boolean {
  return /(?:api[_-]?key|authorization|token|secret|password|credential)/i.test(key);
}
