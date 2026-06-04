import { Transform, type TransformCallback } from "node:stream";

/**
 * Value-AGNOSTIC secret patterns: each matches a SHAPE, not a known value.
 * Correctness must not depend on seeding the redactor with the literal
 * secret — the two real leaks this project hit were a database password that
 * survived a naive `sed` mask (as a substring) and an Anthropic key emitted by
 * `wrangler workflows describe` that the harness NEVER loaded. A value-seeded
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
  // Authorization: Bearer <token> — redact the credential, keep the header name
  // so logs stay legible.
  /\b(Authorization\s*:\s*Bearer)\s+[A-Za-z0-9._~+/=-]{8,}/gi,
  // Anthropic + OpenAI-style prefixed keys.
  /sk-ant-[A-Za-z0-9_-]{16,}/g,
  /sk-[A-Za-z0-9_-]{20,}/g,
  // aex workspace / proxy tokens: apt_… / ant_….
  /\b(?:apt|ant)_[A-Za-z0-9_-]{16,}/g,
  // Slack tokens.
  /xox[pbar]-[A-Za-z0-9-]{10,}/g,
  // AWS access key id + secret access key shapes.
  /\b(?:AKIA|ASIA)[A-Z0-9]{16}\b/g,
  /\baws_secret_access_key["'\s:=]+[A-Za-z0-9/+=]{40}/gi,
  // JWT-shaped: header.payload.signature, each base64url, header starts `eyJ`.
  /\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}/g,
  // Keyword-introduced secret assignments (bearer/token/api_key/…).
  /(?:bearer|token|api[_-]?key|access[_-]?token|refresh[_-]?token|client[_-]?secret)["'\s:=]+[A-Za-z0-9_\-./+=]{12,}/gi
];

const REDACTED = "[REDACTED]";

/**
 * Generic high-entropy token catch-all, applied AFTER the named patterns.
 * Catches the secret nobody recognised: any long base64url/hex-ish run that is
 * BOTH high-entropy AND character-class-diverse.
 *
 * The candidate run deliberately EXCLUDES `_`: secrets are contiguous opaque
 * runs, whereas `SCREAMING_SNAKE_CASE` env names and `snake_case` identifiers
 * are `_`-separated words — excluding `_` breaks those into sub-24 fragments so
 * they are never eaten (the `CF_CONFORMANCE_PROBE_URL` false positive). The
 * class-diversity gate (≥2 of lower/upper/digit) then keeps the remaining
 * single-class runs (all-lowercase module paths like `internal/modules/esm`)
 * while still catching mixed-case/alnum secret blobs. Entropy alone cannot
 * separate these — `the_quick…` (4.16) out-scores a real key prefix.
 */
const HIGH_ENTROPY_CANDIDATE = /[A-Za-z0-9+/=-]{24,}/g;
const ENTROPY_BITS_PER_CHAR = 3.0;
const MIN_CHAR_CLASSES = 2;
/**
 * A mixed-case run with no digit and < this length is treated as a benign
 * identifier, not a secret. Real opaque secrets are alnum-mixed (carry a
 * digit) or very long; digit-free camelCase identifiers like
 * `asyncRunEntryPointWithESMLoader` / `createDurableObjectNamespace` (which
 * appear in stack traces the diagnostic bundle captures) are 24–39 chars and
 * digit-free — eating them would gut debuggability. The length escape hatch
 * still catches the rare long digit-free secret.
 */
const HIGH_ENTROPY_NO_DIGIT_MIN_LEN = 40;

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
  out = SECRET_PATTERNS.reduce(
    (current, pattern) =>
      current.replace(pattern, (match, captured?: string) =>
        // Patterns with a captured prefix (Authorization header) keep the
        // prefix; the rest replace the whole match.
        typeof captured === "string" ? `${captured} ${REDACTED}` : REDACTED
      ),
    out
  );
  return out.replace(HIGH_ENTROPY_CANDIDATE, (match) =>
    looksHighEntropySecret(match) ? REDACTED : match
  );
}

export function containsSecretLikeValue(input: string): boolean {
  if (
    SECRET_PATTERNS.some((pattern) => {
      pattern.lastIndex = 0;
      return pattern.test(input);
    })
  ) {
    return true;
  }
  HIGH_ENTROPY_CANDIDATE.lastIndex = 0;
  let candidate: RegExpExecArray | null;
  while ((candidate = HIGH_ENTROPY_CANDIDATE.exec(input)) !== null) {
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

/** A candidate run is a high-entropy secret if it is both information-dense
 * AND mixes character classes (the property that separates opaque secret blobs
 * from single-class identifiers/paths). */
function looksHighEntropySecret(value: string): boolean {
  if (charClassCount(value) < MIN_CHAR_CLASSES) return false;
  if (shannonEntropyBits(value) < ENTROPY_BITS_PER_CHAR) return false;
  // Require a digit OR extreme length so digit-free mixed-case identifiers
  // (stack-trace frames, API symbol names) are not destroyed in diagnostic
  // output, while real secret shapes (alnum-mixed, or very long) still match.
  return /[0-9]/.test(value) || value.length >= HIGH_ENTROPY_NO_DIGIT_MIN_LEN;
}

/** How many of {lowercase, uppercase, digit} appear in `value` (0–3). */
function charClassCount(value: string): number {
  let count = 0;
  if (/[a-z]/.test(value)) count++;
  if (/[A-Z]/.test(value)) count++;
  if (/[0-9]/.test(value)) count++;
  return count;
}

/** Shannon entropy in bits/char — the per-character information density. */
function shannonEntropyBits(value: string): number {
  if (value.length === 0) {
    return 0;
  }
  const counts = new Map<string, number>();
  for (const char of value) {
    counts.set(char, (counts.get(char) ?? 0) + 1);
  }
  let bits = 0;
  for (const count of counts.values()) {
    const p = count / value.length;
    bits -= p * Math.log2(p);
  }
  return bits;
}
