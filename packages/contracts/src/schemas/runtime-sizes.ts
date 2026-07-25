/**
 * Schemas for the two sizing-and-lifetime fields a session carries: the
 * `runtimeSize` preset token and the `timeout` duration string.
 *
 * The preset tokens and the accepted-timeout bounds are declared here because
 * they are what the schemas assert; `runtime-sizes.ts` imports this module and
 * re-exports them, so declaring them there instead would be a cycle whose
 * evaluation order decides between an initialised constant and a
 * temporal-dead-zone error. The resource table each token maps to
 * (`RUNTIME_SIZE_PRESETS`) stays on the product side and is proved exhaustive
 * against these tokens at compile time.
 */
import * as z from "zod/mini";

/** The accepted runtime-size values (the wire/CLI tokens), ordered as offered. */
export const RUNTIME_SIZES = [
  "0.25cpu-1gb",
  "0.5cpu-4gb",
  "1cpu-6gb",
  "2cpu-8gb",
  "4cpu-12gb"
] as const;

/**
 * Wire shape of `runtimeSize`. Omission is handled by `parseRuntimeSize`, which
 * leaves the default to the consumer rather than landing one on the request.
 */
export const RuntimeSizeSchema = z.enum(RUNTIME_SIZES, {
  error: (issue) =>
    `runtimeSize must be one of: ${RUNTIME_SIZES.join(", ")} (got ${JSON.stringify(issue.input)})`
});

// ===========================================================================
// Session timeout
// ===========================================================================

/**
 * Hard ceiling for an explicitly supplied session deadline (8 hours).
 * This bounds accepted customer input and lifetime reasoning; it is not the
 * omission default (`DEFAULT_SESSION_TIMEOUT_MS`) even while both policies
 * currently resolve to eight hours. Bounds on accepted input belong to the
 * schema; the omission default is policy and stays with the resolver.
 */
export const MAX_SESSION_TIMEOUT_MS = 8 * 60 * 60 * 1000;

/** Floor on a session deadline (1 minute). */
export const MIN_SESSION_TIMEOUT_MS = 60 * 1000;

/**
 * The duration grammar: a non-negative magnitude and an optional unit, where a
 * bare magnitude means milliseconds. Surrounding whitespace is tolerated.
 */
const DURATION_PATTERN = /^(\d+(?:\.\d+)?)(ms|s|m|h)?$/;

const UNIT_FACTOR_MS = { ms: 1, s: 1_000, m: 60_000, h: 3_600_000 } as const;

/**
 * The magnitude and unit factor a duration string names, or `undefined` when it
 * does not match the grammar. A bare magnitude means milliseconds.
 */
function durationParts(value: string): { magnitude: number; factorMs: number } | undefined {
  const match = DURATION_PATTERN.exec(value.trim());
  if (match === null) {
    return undefined;
  }
  return {
    magnitude: Number(match[1]),
    factorMs: UNIT_FACTOR_MS[(match[2] ?? "ms") as keyof typeof UNIT_FACTOR_MS]
  };
}

function malformed(input: unknown): string {
  return `invalid duration ${JSON.stringify(input)} (expected e.g. "1h", "90m", "30s", "500ms", or a bare ms integer)`;
}

/**
 * A human duration string (`"1h"`, `"90m"`, `"3600s"`, `"500ms"`, or a bare-ms
 * integer).
 *
 * Grammar and magnitude are two checks, not one, because they fail for
 * different reasons and say so: a string that does not match the grammar at all
 * is malformed, while one that matches but names a magnitude too large to be a
 * finite number is a number problem. Both abort so the first failure is the
 * reported one.
 */
export const DurationSchema = z
  .string({ error: (issue) => malformed(issue.input) })
  .check(
    z.refine((value: string) => durationParts(value) !== undefined, {
      error: (issue) => malformed(issue.input),
      abort: true
    }),
    z.refine(
      (value: string) => {
        const parts = durationParts(value);
        return parts !== undefined && Number.isFinite(parts.magnitude) && parts.magnitude >= 0;
      },
      {
        error: (issue) => `invalid duration ${JSON.stringify(issue.input)} (must be a non-negative number)`,
        abort: true
      }
    )
  );

/**
 * Convert a validated duration string to whole milliseconds.
 *
 * Deliberately NOT a `.transform()` on {@link DurationSchema}: `z.toJSONSchema(s,
 * {io:"output"})` throws on any transform, which would make the response half of
 * the generated spec ungenerable (L1). Schemas validate; `normalize*()`
 * functions transform. Input is assumed to have passed {@link DurationSchema}.
 */
export function normalizeDurationToMs(duration: string): number {
  const parts = durationParts(duration);
  if (parts === undefined) {
    // Unreachable through `parseDurationToMs`, which validates first. Raising
    // the schema's own wording rather than returning NaN keeps a direct caller
    // from propagating a silently bad number, and keeps one statement of the
    // message.
    throw new Error(malformed(duration));
  }
  return Math.round(parts.magnitude * parts.factorMs);
}

/**
 * Wire shape of `timeout` — a duration string, rejected by type before the
 * grammar is consulted so a caller who sent a number learns that first.
 */
export const SessionTimeoutSchema = z.string({
  error: (issue) =>
    `timeout must be a duration string (e.g. "1h", "30m"); got ${JSON.stringify(issue.input)}`
});

/**
 * The accepted window for a resolved session deadline.
 *
 * A bound on the decoded milliseconds rather than on the duration string, since
 * `"90m"` and `"5400000"` are the same deadline and must be judged the same
 * way. Invisible in the generated spec (L2) — a JSON Schema for a string cannot
 * state a bound on what the string decodes to — which is why it is named here.
 */
export const SessionTimeoutMsSchema = z.number().check(
  z.refine((ms: number) => ms >= MIN_SESSION_TIMEOUT_MS, {
    error: (issue) =>
      `timeout must be at least ${MIN_SESSION_TIMEOUT_MS}ms (1m); got ${issue.input as number}ms`,
    abort: true
  }),
  z.refine((ms: number) => ms <= MAX_SESSION_TIMEOUT_MS, {
    error: (issue) =>
      `timeout must be at most ${MAX_SESSION_TIMEOUT_MS}ms (8h); got ${issue.input as number}ms`,
    abort: true
  })
);
