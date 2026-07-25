/**
 * Schema for the managed execution-runtime selector.
 *
 * The token set lives here rather than beside the product documentation in
 * `runtime-kind.ts` for one reason: the schema and the vocabulary must be the
 * same statement. A schema built from a list declared elsewhere would have to
 * import it, and `runtime-kind.ts` already imports this module — a cycle whose
 * evaluation order decides whether the enum sees an initialised list or a
 * temporal-dead-zone error. Declaring the tokens where the schema is built
 * removes the question; `runtime-kind.ts` re-exports them unchanged.
 */
import * as z from "zod/mini";

/** The accepted execution-runtime values (the wire/CLI tokens). */
export const RUNTIME_KINDS = ["container", "spot_container", "lambda"] as const;

/**
 * Wire shape of `runtimeKind`.
 *
 * Omission is the caller's signal to take {@link import("../runtime-kind.js").DEFAULT_RUNTIME_KIND},
 * and it is handled by `parseRuntimeKind` rather than by an `optional()` here:
 * the field is required once present, and a defaulted schema would describe a
 * value the wire never carries.
 *
 * One diagnostic covers every rejection — wrong type, wrong case, unknown token
 * — because that is the only distinction a caller can act on: the message names
 * the whole menu.
 */
export const RuntimeKindSchema = z.enum(RUNTIME_KINDS, {
  error: (issue) =>
    `runtimeKind must be one of: ${RUNTIME_KINDS.join(", ")} (got ${JSON.stringify(issue.input)})`
});
