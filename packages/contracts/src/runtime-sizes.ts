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
 * Tokens intentionally remain stable product presets.
 */
export const RUNTIME_SIZE_PRESETS = {
  "shared-1x-128mb": { cpus: 1, memoryMb: 128 },
  "shared-1x-256mb": { cpus: 1, memoryMb: 256 },
  "shared-1x-512mb": { cpus: 1, memoryMb: 512 },
  "shared-1x-1gb": { cpus: 1, memoryMb: 1024 },
  "shared-1x-2gb": { cpus: 1, memoryMb: 2048 },
  "shared-2x-512mb": { cpus: 2, memoryMb: 512 },
  "shared-2x-1gb": { cpus: 2, memoryMb: 1024 },
  "shared-2x-2gb": { cpus: 2, memoryMb: 2048 },
  "shared-2x-4gb": { cpus: 2, memoryMb: 4096 },
  "shared-4x-1gb": { cpus: 4, memoryMb: 1024 },
  "shared-4x-2gb": { cpus: 4, memoryMb: 2048 },
  "shared-4x-4gb": { cpus: 4, memoryMb: 4096 },
  "shared-4x-8gb": { cpus: 4, memoryMb: 8192 },
  "shared-8x-2gb": { cpus: 8, memoryMb: 2048 },
  "shared-8x-4gb": { cpus: 8, memoryMb: 4096 },
  "shared-8x-8gb": { cpus: 8, memoryMb: 8192 },
  "shared-8x-16gb": { cpus: 8, memoryMb: 16384 }
} as const satisfies Record<string, RuntimeResources>;

/** The accepted runtime-size values (the wire/CLI tokens). */
export type RuntimeSize = keyof typeof RUNTIME_SIZE_PRESETS;

/** All preset tokens, ordered as declared. Handy for CLI help + validation. */
export const RUNTIME_SIZES = Object.keys(RUNTIME_SIZE_PRESETS) as readonly RuntimeSize[];

/** Default when `runtimeSize` is omitted. */
export const DEFAULT_RUNTIME_SIZE: RuntimeSize = "shared-1x-128mb";

/**
 * Symbol-style accessors for TS callers. `RuntimeSizes.SHARED_2X_2GB`
 * resolves to the wire token `"shared-2x-2gb"`.
 */
export const RuntimeSizes = {
  SHARED_1X_128MB: "shared-1x-128mb",
  SHARED_1X_256MB: "shared-1x-256mb",
  SHARED_1X_512MB: "shared-1x-512mb",
  SHARED_1X_1GB: "shared-1x-1gb",
  SHARED_1X_2GB: "shared-1x-2gb",
  SHARED_2X_512MB: "shared-2x-512mb",
  SHARED_2X_1GB: "shared-2x-1gb",
  SHARED_2X_2GB: "shared-2x-2gb",
  SHARED_2X_4GB: "shared-2x-4gb",
  SHARED_4X_1GB: "shared-4x-1gb",
  SHARED_4X_2GB: "shared-4x-2gb",
  SHARED_4X_4GB: "shared-4x-4gb",
  SHARED_4X_8GB: "shared-4x-8gb",
  SHARED_8X_2GB: "shared-8x-2gb",
  SHARED_8X_4GB: "shared-8x-4gb",
  SHARED_8X_8GB: "shared-8x-8gb",
  SHARED_8X_16GB: "shared-8x-16gb"
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

/** Default run deadline when `timeout` is omitted (1 hour). */
export const DEFAULT_RUN_TIMEOUT_MS = 60 * 60 * 1000;

/** Hard ceiling on a run deadline (6 hours). */
export const MAX_RUN_TIMEOUT_MS = 6 * 60 * 60 * 1000;

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
    throw new Error(`timeout must be at most ${MAX_RUN_TIMEOUT_MS}ms (6h); got ${ms}ms`);
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
