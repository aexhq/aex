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
 *   - `container`      — today's co-located Fargate (on-demand). Explicit opt-in.
 *   - `spot_container` — the same behavior on Fargate Spot: cheaper and
 *                        interruption tolerant through durable recovery. This is
 *                        {@link DEFAULT_RUNTIME_KIND}.
 *   - `lambda`         — event-driven Lambda + on-demand MicroVM sandbox;
 *                        idle → $0, fast resume, ≤32 GB workspace.
 *
 * The wire shape of a submission is identical across all three, but the runtimes
 * are NOT yet capability-equivalent: `lambda` has a 900 s per-invocation ceiling, a
 * 32 GiB / 8 h sandbox, and no tool execution until the MicroVM hands channel ships.
 * `spot_container` executes side-effecting tools at-least-once because a Spot
 * reclaim replays the interrupted step. Pricing differs per runtime.
 */

import { rethrowContractParseError } from "./contract-parse-error.js";
import { RUNTIME_KINDS, RuntimeKindSchema } from "./schemas/runtime-kind.js";
import { parseWire } from "./schemas/wire.js";

/** The accepted execution-runtime values (the wire/CLI tokens). */
export { RUNTIME_KINDS };

/** One of the closed {@link RUNTIME_KINDS} tokens. */
export type RuntimeKind = (typeof RUNTIME_KINDS)[number];

/**
 * Default when `runtimeKind` is omitted: `spot_container` (Fargate Spot).
 *
 * Chosen because it is the cheapest kind that can execute every tool today. The
 * `lambda` path can finish an LLM turn but cannot execute a tool call, so it is
 * an explicit opt-in rather than the default. Kept byte-equal to the hosted
 * declaration in `packages/contract-core/src/runtime-kind.ts`, which carries the
 * revert condition.
 */
export const DEFAULT_RUNTIME_KIND: RuntimeKind = "spot_container";

/**
 * Symbol-style accessors for TS callers: `RuntimeKinds.SPOT_CONTAINER` resolves
 * to the wire token `"spot_container"`. Re-exported by the SDK as
 * `RuntimeKinds`. Prefer these over raw strings so an invalid token is a compile
 * error, not a runtime 400.
 */
export const RuntimeKinds = {
  /** Today's co-located Fargate, on-demand capacity. Explicit opt-in. */
  CONTAINER: "container",
  /**
   * Fargate Spot — same behavior, cheaper capacity, interruption tolerant.
   * This is {@link DEFAULT_RUNTIME_KIND}: what an omitted `runtime.kind` resolves to.
   */
  SPOT_CONTAINER: "spot_container",
  /** Event-driven Lambda + on-demand MicroVM sandbox; idle → $0. Explicit opt-in. */
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
    return parseWire(RuntimeKindSchema, input);
  } catch (error) {
    rethrowContractParseError(error, "parseRuntimeKind");
  }
}
