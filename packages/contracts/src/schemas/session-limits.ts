/**
 * Schema for the per-session lineage-limit override.
 *
 * A SHAPE/positivity gate only. Clamping to the workspace and platform ceilings
 * is the resolver's job (`resolveSessionLimits` in `@aexhq/shared`) and must not
 * migrate here — a schema that clamped would silently change a caller's request
 * instead of describing it.
 */
import * as z from "zod/mini";
import { positiveInt, positiveNumber } from "./numeric.js";
import { wireObject, type Present } from "./wire.js";

/**
 * Wire shape of `limits`. Declaration order is the permitted-key order reported
 * on an unknown field, and the order {@link normalizeSessionLimits} emits.
 */
export const SessionLimitsSchema = wireObject("limits", {
  maxConcurrentChildSessions: z.optional(positiveInt("limits.maxConcurrentChildSessions")),
  maxSubagentDepth: z.optional(positiveInt("limits.maxSubagentDepth")),
  maxSpendUsd: z.optional(positiveNumber("limits.maxSpendUsd")),
  maxTurns: z.optional(positiveInt("limits.maxTurns")),
  maxStepsPerTurn: z.optional(positiveInt("limits.maxStepsPerTurn"))
});

export type SessionLimitsWire = z.infer<typeof SessionLimitsSchema>;

/**
 * Collapse a no-signal override to `undefined` and drop absent fields.
 *
 * Deliberately NOT a `.transform()` on the schema: `z.toJSONSchema(s, {io:"output"})`
 * throws on any transform, which would make the response half of the generated
 * spec ungenerable (L1). Schemas validate; `normalize*()` functions transform.
 *
 * `limits: {}` carries no more signal than an absent `limits`, and landing an
 * empty object on the request would make the two distinguishable downstream for
 * no reason. The resolver supplies platform defaults for every absent field.
 */
export function normalizeSessionLimits(
  limits: SessionLimitsWire
): Present<SessionLimitsWire> | undefined {
  const present = Object.entries(limits).filter(([, value]) => value !== undefined);
  return present.length === 0
    ? undefined
    : (Object.fromEntries(present) as Present<SessionLimitsWire>);
}
