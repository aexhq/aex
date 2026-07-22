/**
 * Managed execution-runtime selector ("which backend runs the session").
 *
 * Distinct from {@link import("./runtime-sizes.js").RuntimeSize} ("how big the
 * box is"). A session picks BOTH: a `runtimeKind` (this file) and a
 * `runtimeSize`. The kind selects the execution backend; the size selects the
 * vCPU/memory preset within it.
 *
 * Three product runtimes, selected per session (feature addition, not a
 * migration — see `references/microvm-migration-2026-07-15/00`§0):
 *   - `container`      — today's co-located Fargate (on-demand). Default.
 *   - `spot_container` — the same behavior on Fargate Spot: cheaper and
 *                        interruption tolerant through durable recovery.
 *   - `lambda`         — event-driven Lambda + on-demand MicroVM sandbox;
 *                        idle → $0, fast resume, ≤32 GB workspace.
 *
 * The customer-facing surface is identical across all three; only the
 * `runtimeKind` selector and the per-runtime pricing differ.
 */

/** The accepted execution-runtime values (the wire/CLI tokens). */
import { rethrowContractParseError } from "./contract-parse-error.js";

export const RUNTIME_KINDS = ["container", "spot_container", "lambda"] as const;

/** One of the closed {@link RUNTIME_KINDS} tokens. */
export type RuntimeKind = (typeof RUNTIME_KINDS)[number];

/** Default when `runtimeKind` is omitted (today's co-located Fargate path). */
export const DEFAULT_RUNTIME_KIND: RuntimeKind = "container";

/**
 * Symbol-style accessors for TS callers: `RuntimeKinds.SPOT_CONTAINER` resolves
 * to the wire token `"spot_container"`. Re-exported by the SDK as
 * `RuntimeKinds`. Prefer these over raw strings so an invalid token is a compile
 * error, not a runtime 400.
 */
export const RuntimeKinds = {
  /** Today's co-located Fargate (on-demand). The default. */
  CONTAINER: "container",
  /** Fargate Spot — same behavior, cheaper capacity, interruption tolerant. */
  SPOT_CONTAINER: "spot_container",
  /** Event-driven Lambda + on-demand MicroVM sandbox; idle → $0. */
  LAMBDA: "lambda"
} as const satisfies Record<string, RuntimeKind>;

/**
 * Validate the wire `runtimeKind` field. `undefined` (omitted) is allowed and
 * consumers apply {@link DEFAULT_RUNTIME_KIND}.
 */
export function parseRuntimeKind(input: unknown): RuntimeKind | undefined {
  try {
    if (input === undefined) {
      return undefined;
    }
    if (typeof input !== "string" || !(RUNTIME_KINDS as readonly string[]).includes(input)) {
      throw new Error(
        `runtimeKind must be one of: ${RUNTIME_KINDS.join(", ")} (got ${JSON.stringify(input)})`
      );
    }
    return input as RuntimeKind;
  } catch (error) {
    rethrowContractParseError(error, "parseRuntimeKind");
  }
}
