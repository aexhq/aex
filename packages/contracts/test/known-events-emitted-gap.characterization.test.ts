/**
 * Characterization — Phase 0 of the event-system rebuild.
 *
 * Locks defect (a): the public `is*Event` type guards exported from
 * `known-events.ts` narrow on the RAW Claude Managed Agents vocabulary
 * (`agent.message`, `agent.tool_use`, `session.status_idle`, …), but a
 * consumer of the antpath stream never sees those strings. The unified
 * `RunnerEvent.kind` (and therefore the `RunEvent.type` the SDK receives
 * from `GET /api/runs/:id/events`, which maps `type = kind`) is one of
 * `RUNNER_EVENT_KINDS` (`assistant_text`, `tool_request`, …).
 *
 * So the shipped guards are dead for the emitted stream: applied to a real
 * emitted event they always return false. This test pins that gap exactly.
 * Phase 1 introduces honest guards over the emitted vocabulary and adds the
 * positive tests; this characterization documents the pre-rebuild behaviour
 * the guards had so the change is visible as a test diff.
 *
 * See docs/event-system-build-design notes (Phase 0 / Phase 1).
 */
import { describe, expect, it } from "vitest";
import {
  RUNNER_EVENT_KINDS,
  isAgentMessage,
  isAgentToolUse,
  isAgentToolResult,
  isAgentThinking,
  isSessionStatusIdle,
  isSessionStatusTerminated
} from "../src/index.js";

// The shape a consumer actually holds: a RunEvent as served by the hosted API's
// read route, where `type` is the RunnerEvent `kind`.
const emittedRunEvents = RUNNER_EVENT_KINDS.map((kind, i) => ({
  id: `evt_run_${i}`,
  type: kind,
  runId: "run_characterization"
}));

describe("known-events guards vs the emitted vocabulary (defect a)", () => {
  it("DEFECT: no shipped is*Event guard recognises ANY emitted RunnerEvent kind", () => {
    // Every emitted event carries `type` ∈ RUNNER_EVENT_KINDS. The guards
    // check `agent.*` / `session.*` raw provider types, so they match none.
    for (const evt of emittedRunEvents) {
      expect(isAgentMessage(evt)).toBe(false);
      expect(isAgentToolUse(evt)).toBe(false);
      expect(isAgentToolResult(evt)).toBe(false);
      expect(isAgentThinking(evt)).toBe(false);
      expect(isSessionStatusIdle(evt)).toBe(false);
      expect(isSessionStatusTerminated(evt)).toBe(false);
    }
  });

  it("DEFECT: the closest semantic guards miss their emitted counterparts", () => {
    // assistant_text is the emitted "an assistant said something"; the guard
    // a consumer would reach for (isAgentMessage) is keyed on `agent.message`.
    expect(isAgentMessage({ type: "assistant_text" })).toBe(false);
    // tool_request is the emitted "model invoked a tool"; isAgentToolUse is
    // keyed on `agent.tool_use`.
    expect(isAgentToolUse({ type: "tool_request" })).toBe(false);
    // runtime_terminal is the emitted "run reached a terminal state"; no
    // session.* guard recognises it.
    expect(isSessionStatusIdle({ type: "runtime_terminal" })).toBe(false);
    expect(isSessionStatusTerminated({ type: "runtime_terminal" })).toBe(false);
  });

  it("the guards DO still recognise the raw provider vocabulary they were built for", () => {
    // Pin the working half: the guards are correct for the raw upstream
    // events the Anthropic adapter consumes — they are simply never applied
    // to those upstream events by any consumer of the unified stream.
    expect(isAgentMessage({ type: "agent.message" })).toBe(true);
    expect(isAgentToolUse({ type: "agent.tool_use" })).toBe(true);
    expect(isSessionStatusIdle({ type: "session.status_idle" })).toBe(true);
  });
});
