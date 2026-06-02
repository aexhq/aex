import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  isAgentEvent,
  isAgentMessage,
  isSessionEvent,
  isSessionStatusIdle,
  isSessionStatusRunning,
  isUserMessage,
  type ProviderEvent
} from "@antpath/contracts";
import { findResidualSecrets, parseSanitizedFixture } from "../../../scripts/lib/fixtures.js";

/**
 * SDK-side record-replay resilience (Tier 0 / unit): the committed sanitized
 * fixture parses cleanly, carries NO secret, and its event-type vocabulary is
 * recognized by the SHARED `is*Event` type guards the SDK ships
 * (packages/contracts/src/known-events.ts). The SDK's stake in record-replay is
 * those guards — if the provider renames a type vocabulary, these guards stop
 * narrowing it and downstream consumers regress. Replaying the fixture through
 * them proves they still recognize the committed shape, offline + $0.
 *
 * Resolves the long-dangling `test:unit:recorded` / `test:load:replay` package
 * scripts (they referenced this file; it was absent on disk).
 */

const FIXTURE_PATH = fileURLToPath(
  new URL("../../fixtures/api-recordings/simple-turn.sanitized.json", import.meta.url)
);

function loadEvents(): ProviderEvent[] {
  const parsed = parseSanitizedFixture(JSON.parse(readFileSync(FIXTURE_PATH, "utf8")));
  return parsed.events as unknown as ProviderEvent[];
}

describe("recorded fixture — SDK type-guard replay", () => {
  it("parses + is secret-free", () => {
    const events = loadEvents();
    expect(events.length).toBeGreaterThan(0);
    expect(findResidualSecrets(events)).toEqual([]);
  });

  it("every fixture event type is recognized by a shared is*Event guard", () => {
    for (const event of loadEvents()) {
      const recognized =
        isUserEventLike(event) ||
        isAgentEvent(event) ||
        isSessionEvent(event) ||
        event.type.startsWith("span.");
      expect(recognized, `unrecognized event type "${event.type}"`).toBe(true);
    }
  });

  it("the documented simple-turn anchors narrow correctly", () => {
    const events = loadEvents();
    expect(events.some((e) => isSessionStatusRunning(e))).toBe(true);
    expect(events.some((e) => isAgentMessage(e))).toBe(true);
    expect(events.some((e) => isSessionStatusIdle(e))).toBe(true);
    expect(events.some((e) => isUserMessage(e))).toBe(true);
  });
});

function isUserEventLike(event: ProviderEvent): boolean {
  return typeof event.type === "string" && event.type.startsWith("user.");
}
