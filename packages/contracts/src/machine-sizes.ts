/**
 * Goose managed-runtime sizing — a CLOSED set of valid `cpu × memory` presets.
 *
 * The managed runtime accepts shared-CPU guests only at specific `(cpus, memory_mb)`
 * combinations: memory must be a multiple of 256MB, with
 * `min = 256 * cpus` and `max = 2048 * cpus`. Rather than accept a free-form
 * `{ cpu, memory }` and validate it (which can still produce a combo the host
 * rejects at machine-create time), we expose only this pre-generated set of
 * known-good presets. The type system then makes an invalid pairing
 * *unrepresentable* — the whole "host rejected the guest size" error class is
 * gone at compile time.
 *
 * Wire/CLI value is the kebab token (`"shared-2x-2gb"`). TS callers prefer
 * the {@link MachineSizes} symbol const (`MachineSizes.SHARED_2X_2GB`) so a
 * typo is a compile error, not a runtime 400.
 *
 * Native (Anthropic Managed Agents) runs have no managed host machine and ignore the
 * `machine` field entirely; it only takes effect on the Goose runtime.
 */

export interface MachineResources {
  readonly cpus: number;
  readonly memoryMb: number;
}

/**
 * The single source of truth: every offered preset, keyed by its wire token.
 * All 16 entries sit inside Fly's valid shared-CPU range
 * (256MB ≤ memory ≤ 2048MB × cpus, multiples of 256MB). The `satisfies`
 * clause keeps the value shape honest while preserving the literal key union.
 */
export const MACHINE_PRESETS = {
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
} as const satisfies Record<string, MachineResources>;

/** The only accepted machine-size values (the wire/CLI tokens). */
export type MachineSize = keyof typeof MACHINE_PRESETS;

/** All preset tokens, ordered as declared. Handy for CLI help + validation. */
export const MACHINE_SIZES = Object.keys(MACHINE_PRESETS) as readonly MachineSize[];

/**
 * Default when `machine` is omitted — 1 shared CPU / 512MB, i.e. the exact
 * resources the Goose runner used before this field existed. Keeps existing
 * runs byte-for-byte unchanged.
 */
export const DEFAULT_MACHINE_SIZE: MachineSize = "shared-1x-512mb";

/**
 * Symbol-style accessors for TS callers. `MachineSizes.SHARED_2X_2GB`
 * resolves to the wire token `"shared-2x-2gb"`; referencing a preset that
 * doesn't exist is a compile error (no such property), which is friendlier
 * than relying on the caller to type the kebab token correctly. Kept in
 * lockstep with {@link MACHINE_PRESETS} by `machine-sizes.test.ts`.
 */
export const MachineSizes = {
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
} as const satisfies Record<string, MachineSize>;

/** Resolve a preset token to its concrete Fly guest resources. */
export function machineResources(size: MachineSize): MachineResources {
  return MACHINE_PRESETS[size];
}

/**
 * Validate the wire `machine` field. `undefined` (omitted) is allowed —
 * the consumer falls back to {@link DEFAULT_MACHINE_SIZE}. Any other value
 * must be one of the closed preset set; an unknown token fails loud with the
 * full list so the caller can correct it.
 */
export function parseMachineSize(input: unknown): MachineSize | undefined {
  if (input === undefined) {
    return undefined;
  }
  if (typeof input !== "string" || !(MACHINE_SIZES as readonly string[]).includes(input)) {
    throw new Error(
      `machine must be one of: ${MACHINE_SIZES.join(", ")} (got ${JSON.stringify(input)})`
    );
  }
  return input as MachineSize;
}

// ===========================================================================
// Run timeout
// ===========================================================================

/** Default run deadline when `timeout` is omitted (1 hour). */
export const DEFAULT_RUN_TIMEOUT_MS = 60 * 60 * 1000;

/** Hard ceiling on a run deadline (6 hours) — bounds runaway cost. */
export const MAX_RUN_TIMEOUT_MS = 6 * 60 * 60 * 1000;

/** Floor on a run deadline (1 minute) — below this is almost always a typo. */
export const MIN_RUN_TIMEOUT_MS = 60 * 1000;

const DURATION_PATTERN = /^(\d+(?:\.\d+)?)(ms|s|m|h)?$/;

/**
 * Parse a human duration string (`"1h"`, `"90m"`, `"3600s"`, `"500ms"`, or a
 * bare-ms integer) into milliseconds. Throws on malformed input. Shared by
 * the SDK/CLI surface and the submission parser so one grammar governs every
 * duration the platform accepts.
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
 * {@link DEFAULT_RUN_TIMEOUT_MS}. Out-of-range values fail loud at submit
 * time so the caller gets a 400, not a silently-clamped run.
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

/** Apply the default when a parsed `timeoutMs` is absent. Single source of the default. */
export function resolveRunTimeoutMs(timeoutMs: number | undefined): number {
  return timeoutMs ?? DEFAULT_RUN_TIMEOUT_MS;
}

/** Format a millisecond deadline as the second-granularity string Inngest's `timeout` accepts. */
export function inngestTimeoutString(ms: number): string {
  return `${Math.max(1, Math.ceil(ms / 1000))}s`;
}

// ===========================================================================
// Graceful run termination at the deadline
// ===========================================================================
//
// When a run hits its deadline the stop is staged, not abrupt:
//   1. The runner SIGINTs goose at `timeoutMs` (graceful interrupt).
//   2. If goose hasn't exited {@link RUN_GOOSE_KILL_GRACE_MS} later, the runner
//      SIGKILLs it — so the runner still reaches its outputs-upload + terminal
//      path instead of hanging on a goose that ignored SIGINT.
//   3. The orchestrator waits {@link RUN_GRACE_MS} PAST the deadline for the
//      runner's `run/terminal` before it force-destroys the Fly machine. That
//      headroom is what lets the runner finish the SIGKILL escalation AND the
//      outputs/archive upload before the machine disappears.
//
// Invariant: RUN_GRACE_MS > RUN_GOOSE_KILL_GRACE_MS — the orchestrator must
// outlast the runner's own kill grace, or it would tear the machine down while
// the runner is still uploading. The difference is the upload/terminal budget.

/** Runner: time to wait after SIGINT before SIGKILLing goose at the deadline (1m). */
export const RUN_GOOSE_KILL_GRACE_MS = 60 * 1000;

/**
 * Orchestrator: extra window past `timeoutMs` to wait for the runner's terminal
 * before hard-destroying the Fly machine (90s = 60s goose kill-grace + 30s
 * outputs/archive/terminal upload budget).
 */
export const RUN_GRACE_MS = 90 * 1000;
