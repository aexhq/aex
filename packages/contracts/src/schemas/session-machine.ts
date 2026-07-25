/**
 * Schema for the per-session capacity intent.
 *
 * Intent only — the managed runtime selects capacity from it. `spot: true` opts
 * into interruptible capacity; absent or `false` requests standard capacity.
 */
import * as z from "zod/mini";
import { wireObject, type Present } from "./wire.js";

/** Wire shape of `machine`. */
export const SessionMachineSchema = wireObject("machine", {
  spot: z.optional(z.boolean({ error: "machine.spot must be a boolean" }))
});

export type SessionMachineWire = z.infer<typeof SessionMachineSchema>;

/**
 * Collapse a no-signal object (`machine: {}`) to `undefined`; preserve an
 * explicit `spot`, including `false`.
 *
 * Separate from the schema for the same reason as the sibling limits
 * normaliser — see {@link import("./session-limits.js").normalizeSessionLimits}.
 * `false` is signal here: it is a caller explicitly declining interruptible
 * capacity, which is not the same as not having asked.
 */
export function normalizeSessionMachine(
  machine: SessionMachineWire
): Present<SessionMachineWire> | undefined {
  return machine.spot === undefined ? undefined : { spot: machine.spot };
}
