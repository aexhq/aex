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
 * (`shared-0.06x-256mb`) tier is for light / IO-bound runs only.
 */
export const RUNTIME_SIZE_PRESETS = {
  "shared-0.06x-256mb": { cpus: 0.0625, memoryMb: 256 },
  "shared-0.25x-1gb": { cpus: 0.25, memoryMb: 1024 },
  "shared-0.5x-4gb": { cpus: 0.5, memoryMb: 4096 },
  "shared-1x-6gb": { cpus: 1, memoryMb: 6144 },
  "shared-2x-8gb": { cpus: 2, memoryMb: 8192 },
  "shared-4x-12gb": { cpus: 4, memoryMb: 12288 }
} as const satisfies Record<string, RuntimeResources>;

/** The accepted runtime-size values (the wire/CLI tokens). */
export type RuntimeSize = keyof typeof RUNTIME_SIZE_PRESETS;

/** All preset tokens, ordered as declared. Handy for CLI help + validation. */
export const RUNTIME_SIZES = Object.keys(RUNTIME_SIZE_PRESETS) as readonly RuntimeSize[];

/** Default when `runtimeSize` is omitted (the 1 GB tier). */
export const DEFAULT_RUNTIME_SIZE: RuntimeSize = "shared-0.25x-1gb";

/**
 * Symbol-style accessors for TS callers: the `SHARED_2X_8GB` member resolves to
 * the wire token `"shared-2x-8gb"`. Re-exported by the SDK as `Sizes`.
 */
export const RuntimeSizes = {
  SHARED_0_06X_256MB: "shared-0.06x-256mb",
  SHARED_0_25X_1GB: "shared-0.25x-1gb",
  SHARED_0_5X_4GB: "shared-0.5x-4gb",
  SHARED_1X_6GB: "shared-1x-6gb",
  SHARED_2X_8GB: "shared-2x-8gb",
  SHARED_4X_12GB: "shared-4x-12gb"
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
// Run timeout
// ===========================================================================

/** Default run deadline when `timeout` is omitted (8 hours). */
export const DEFAULT_RUN_TIMEOUT_MS = 8 * 60 * 60 * 1000;

/** Hard ceiling on a run deadline (8 hours). */
export const MAX_RUN_TIMEOUT_MS = 8 * 60 * 60 * 1000;

/** Floor on a run deadline (1 minute). */
export const MIN_RUN_TIMEOUT_MS = 60 * 1000;

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
 * {@link DEFAULT_RUN_TIMEOUT_MS}.
 */
export function parseRunTimeout(input: unknown): number | undefined {
  if (input === undefined) {
    return undefined;
  }
  if (typeof input !== "string") {
    throw new Error(`timeout must be a duration string (e.g. "1h", "30m"); got ${JSON.stringify(input)}`);
  }
  const ms = parseDurationToMs(input);
  if (ms < MIN_RUN_TIMEOUT_MS) {
    throw new Error(`timeout must be at least ${MIN_RUN_TIMEOUT_MS}ms (1m); got ${ms}ms`);
  }
  if (ms > MAX_RUN_TIMEOUT_MS) {
    throw new Error(`timeout must be at most ${MAX_RUN_TIMEOUT_MS}ms (8h); got ${ms}ms`);
  }
  return ms;
}

/** Apply the default when a parsed `timeoutMs` is absent. */
export function resolveRunTimeoutMs(timeoutMs: number | undefined): number {
  return timeoutMs ?? DEFAULT_RUN_TIMEOUT_MS;
}

/** Format a millisecond deadline as a second-granularity duration string. */
export function orchestrationTimeoutString(ms: number): string {
  return `${Math.max(1, Math.ceil(ms / 1000))}s`;
}

/** Runtime process: time to wait after graceful interrupt before hard kill. */
export const RUN_PROCESS_KILL_GRACE_MS = 60 * 1000;

/**
 * Orchestrator: extra window past `timeoutMs` to wait for terminal callback
 * before host-level cleanup.
 */
export const RUN_TERMINAL_GRACE_MS = 90 * 1000;
