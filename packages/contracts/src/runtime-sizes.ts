/**
 * Managed runtime sizing presets.
 *
 * The public contract exposes product-level runtime size tokens, not host
 * implementation details. The closed token set keeps invalid resource pairings
 * out of the wire contract while leaving the concrete host mapping private.
 */

import { withContractParseError } from "./contract-parse-error.js";
import {
  DurationSchema,
  MAX_SESSION_TIMEOUT_MS,
  MIN_SESSION_TIMEOUT_MS,
  RUNTIME_SIZES,
  RuntimeSizeSchema,
  SessionTimeoutMsSchema,
  SessionTimeoutSchema,
  normalizeDurationToMs
} from "./schemas/runtime-sizes.js";
import { parseWire } from "./schemas/wire.js";

export interface RuntimeResources {
  readonly cpus: number;
  readonly memoryMb: number;
}

/** All preset tokens, ordered as declared. Handy for CLI help + validation. */
export { RUNTIME_SIZES };

/** The accepted runtime-size values (the wire/CLI tokens). */
export type RuntimeSize = (typeof RUNTIME_SIZES)[number];

/**
 * The single source of truth for what a preset *costs*: every offered token
 * mapped to its product-level resources. The token vocabulary itself is the
 * schema's (`schemas/runtime-sizes.ts`), and `Record<RuntimeSize, …>` makes any
 * drift between the two a compile error rather than a runtime surprise. The
 * smallest (`0.25cpu-1gb`) tier is also the default.
 */
export const RUNTIME_SIZE_PRESETS = {
  "0.25cpu-1gb": { cpus: 0.25, memoryMb: 1024 },
  "0.5cpu-4gb": { cpus: 0.5, memoryMb: 4096 },
  "1cpu-6gb": { cpus: 1, memoryMb: 6144 },
  "2cpu-8gb": { cpus: 2, memoryMb: 8192 },
  "4cpu-12gb": { cpus: 4, memoryMb: 12288 }
} as const satisfies Record<RuntimeSize, RuntimeResources>;

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
  return withContractParseError("parseRuntimeSize", () => {
    if (input === undefined) return undefined;
    return parseWire(RuntimeSizeSchema, input);
  });
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
 * The accepted-input window, declared with the schema that enforces it. The
 * ceiling is independent from {@link DEFAULT_SESSION_TIMEOUT_MS}, which is the
 * omission policy above and merely has the same value today.
 */
export { MAX_SESSION_TIMEOUT_MS, MIN_SESSION_TIMEOUT_MS };

/**
 * Parse a human duration string (`"1h"`, `"90m"`, `"3600s"`, `"500ms"`, or a
 * bare-ms integer) into milliseconds. Throws on malformed input.
 */
export function parseDurationToMs(input: string): number {
  return withContractParseError("parseDurationToMs", () =>
    normalizeDurationToMs(parseWire(DurationSchema, input))
  );
}

/**
 * Validate the wire `timeout` field (a duration string) into a bounded ms
 * value. `undefined` (omitted) returns `undefined`; the consumer applies
 * {@link DEFAULT_SESSION_TIMEOUT_MS}.
 */
export function parseSessionTimeout(input: unknown): number | undefined {
  return withContractParseError("parseSessionTimeout", () => {
    if (input === undefined) return undefined;
    // Three gates in the caller's reading order: it is a string, it is a
    // duration, it is in range. The middle one stays a `parseDurationToMs` call
    // so a malformed string is still branded by the parser that rejected it.
    return parseWire(
      SessionTimeoutMsSchema,
      parseDurationToMs(parseWire(SessionTimeoutSchema, input))
    );
  });
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
