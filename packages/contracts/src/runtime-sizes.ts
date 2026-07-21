/**
 * Managed runtime sizing presets.
 *
 * The public contract exposes product-level runtime size tokens, not host
 * implementation details. The closed token set keeps invalid resource pairings
 * out of the wire contract while leaving the concrete host mapping private.
 */

export interface RuntimeResources {
  readonly cpus: number;
  readonly memoryMb: number;
}

/**
 * The single source of truth: every offered preset, keyed by its wire token.
 * Tokens intentionally remain stable product presets. The smallest
 * (`0.25cpu-1gb`) tier is also the default.
 */
export const RUNTIME_SIZE_PRESETS = {
  "0.25cpu-1gb": { cpus: 0.25, memoryMb: 1024 },
  "0.5cpu-4gb": { cpus: 0.5, memoryMb: 4096 },
  "1cpu-6gb": { cpus: 1, memoryMb: 6144 },
  "2cpu-8gb": { cpus: 2, memoryMb: 8192 },
  "4cpu-12gb": { cpus: 4, memoryMb: 12288 }
} as const satisfies Record<string, RuntimeResources>;

/** The accepted runtime-size values (the wire/CLI tokens). */
export type RuntimeSize = keyof typeof RUNTIME_SIZE_PRESETS;

/** All preset tokens, ordered as declared. Handy for CLI help + validation. */
export const RUNTIME_SIZES = Object.keys(RUNTIME_SIZE_PRESETS) as readonly RuntimeSize[];

/** Default when `runtimeSize` is omitted (the 1 GB tier). */
export const DEFAULT_RUNTIME_SIZE: RuntimeSize = "0.25cpu-1gb";

/**
 * Symbol-style accessors for TS callers: the `CPU_2_8GB` member resolves to
 * the wire token `"2cpu-8gb"`. Re-exported by the SDK as `Sizes`. The const key
 * is a TS identifier alias (letter-led); it does not mirror the digit-led token.
 */
export const RuntimeSizes = {
  CPU_0_25_1GB: "0.25cpu-1gb",
  CPU_0_5_4GB: "0.5cpu-4gb",
  CPU_1_6GB: "1cpu-6gb",
  CPU_2_8GB: "2cpu-8gb",
  CPU_4_12GB: "4cpu-12gb"
} as const satisfies Record<string, RuntimeSize>;

/** Resolve a preset token to its product-level resource descriptor. */
export function runtimeResources(size: RuntimeSize): RuntimeResources {
  return RUNTIME_SIZE_PRESETS[size];
}

/**
 * Validate the wire `runtimeSize` field. `undefined` (omitted) is allowed and
 * consumers apply {@link DEFAULT_RUNTIME_SIZE}.
 */
export function parseRuntimeSize(input: unknown): RuntimeSize | undefined {
  if (input === undefined) {
    return undefined;
  }
  if (typeof input !== "string" || !(RUNTIME_SIZES as readonly string[]).includes(input)) {
    throw new Error(
      `runtimeSize must be one of: ${RUNTIME_SIZES.join(", ")} (got ${JSON.stringify(input)})`
    );
  }
  return input as RuntimeSize;
}

// ===========================================================================
// Session timeout
// ===========================================================================

/**
 * Session deadline selected only when `timeout` is omitted (8 hours).
 *
 * This is independent from {@link MAX_SESSION_TIMEOUT_MS}: omission policy and
 * the accepted-input ceiling merely have the same value today and may evolve
 * separately. Keep both values explicitly declared rather than aliasing one to
 * the other.
 */
export const DEFAULT_SESSION_TIMEOUT_MS = 8 * 60 * 60 * 1000;

/**
 * Hard ceiling for an explicitly supplied session deadline (8 hours).
 * This bounds accepted customer input and lifetime reasoning; it is not the
 * omission default even while both policies currently resolve to eight hours.
 */
export const MAX_SESSION_TIMEOUT_MS = 8 * 60 * 60 * 1000;

/** Floor on a session deadline (1 minute). */
export const MIN_SESSION_TIMEOUT_MS = 60 * 1000;

const DURATION_PATTERN = /^(\d+(?:\.\d+)?)(ms|s|m|h)?$/;

/**
 * Parse a human duration string (`"1h"`, `"90m"`, `"3600s"`, `"500ms"`, or a
 * bare-ms integer) into milliseconds. Throws on malformed input.
 */
export function parseDurationToMs(input: string): number {
  const match = DURATION_PATTERN.exec(input.trim());
  if (!match) {
    throw new Error(
      `invalid duration ${JSON.stringify(input)} (expected e.g. "1h", "90m", "30s", "500ms", or a bare ms integer)`
    );
  }
  const value = Number(match[1]);
  if (!Number.isFinite(value) || value < 0) {
    throw new Error(`invalid duration ${JSON.stringify(input)} (must be a non-negative number)`);
  }
  const unit = match[2] ?? "ms";
  const factor = unit === "h" ? 3_600_000 : unit === "m" ? 60_000 : unit === "s" ? 1_000 : 1;
  return Math.round(value * factor);
}

/**
 * Validate the wire `timeout` field (a duration string) into a bounded ms
 * value. `undefined` (omitted) returns `undefined`; the consumer applies
 * {@link DEFAULT_SESSION_TIMEOUT_MS}.
 */
export function parseSessionTimeout(input: unknown): number | undefined {
  if (input === undefined) {
    return undefined;
  }
  if (typeof input !== "string") {
    throw new Error(`timeout must be a duration string (e.g. "1h", "30m"); got ${JSON.stringify(input)}`);
  }
  const ms = parseDurationToMs(input);
  if (ms < MIN_SESSION_TIMEOUT_MS) {
    throw new Error(`timeout must be at least ${MIN_SESSION_TIMEOUT_MS}ms (1m); got ${ms}ms`);
  }
  if (ms > MAX_SESSION_TIMEOUT_MS) {
    throw new Error(`timeout must be at most ${MAX_SESSION_TIMEOUT_MS}ms (8h); got ${ms}ms`);
  }
  return ms;
}

/** Apply the default when a parsed `timeoutMs` is absent. */
export function resolveSessionTimeoutMs(timeoutMs: number | undefined): number {
  return timeoutMs ?? DEFAULT_SESSION_TIMEOUT_MS;
}

/** Format a millisecond deadline as a second-granularity duration string. */
export function orchestrationTimeoutString(ms: number): string {
  return `${Math.max(1, Math.ceil(ms / 1000))}s`;
}

/** Runtime process budget after graceful interrupt and before a forced kill. */
export const SESSION_PROCESS_KILL_GRACE_MS = 60 * 1000;

/**
 * Orchestrator budget after `timeoutMs` for a terminal callback before
 * host-level cleanup. This is later and intentionally longer than the runtime
 * process-kill grace.
 */
export const SESSION_TERMINAL_GRACE_MS = 90 * 1000;
