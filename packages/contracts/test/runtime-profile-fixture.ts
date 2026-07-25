import { RUNTIME_CAPABILITY_NAMES, type RuntimeCapabilityState, type RuntimeProfile } from "../src/runtime-types.js";
import { RUNTIME_KINDS, type RuntimeKind } from "../src/runtime-kind.js";

function capabilities(state: RuntimeCapabilityState): RuntimeProfile["capabilities"] {
  return Object.fromEntries(
    RUNTIME_CAPABILITY_NAMES.map((capability) => [capability, state])
  ) as RuntimeProfile["capabilities"];
}

/** One well-formed profile. Kept minimal: the parser, not the values, is under test here. */
export function runtimeProfileFixture(
  runtimeKind: RuntimeKind,
  overrides: Partial<Omit<RuntimeProfile, "runtimeKind" | "schemaVersion">> = {}
): RuntimeProfile {
  return {
    schemaVersion: 1,
    runtimeKind,
    capabilities: capabilities(runtimeKind === "lambda" ? "unsupported" : "supported"),
    limits: {
      maxSessionMs: 28_800_000,
      maxSingleEffectMs: 840_000,
      maxWorkspaceBytes: 20_000_000_000,
      maxConcurrentToolCalls: 1
    },
    delivery: {
      toolExecution: runtimeKind === "spot_container" ? "at-least-once" : "exactly-once",
      coldStartClass: runtimeKind === "lambda" ? "cold-seconds" : "cold-tens-of-seconds",
      idleBilling: runtimeKind === "lambda" ? "zero" : "wall-clock"
    },
    computeBasis: runtimeKind === "lambda" ? "microvm_running" : "wall_clock",
    ...overrides
  };
}

/** The total profile set every `runtimeCapabilities` payload must carry. */
export function runtimeProfilesFixture(): Record<RuntimeKind, RuntimeProfile> {
  return Object.fromEntries(
    RUNTIME_KINDS.map((runtimeKind) => [runtimeKind, runtimeProfileFixture(runtimeKind)])
  ) as Record<RuntimeKind, RuntimeProfile>;
}
