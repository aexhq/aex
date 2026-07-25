import fc from "fast-check";
import { describe, expect, it, setDefaultTimeout } from "bun:test";
import type { FetchLike } from "@aexhq/contracts";
import { Aex } from "../../src/index.js";
import type { AexEvent, JsonValue } from "@aexhq/contracts";

type StreamMode = "parent" | "child";
type EventKind = "CUSTOM" | "RUN_FINISHED" | "RUN_ERROR";

interface EventSpec {
  readonly id: number;
  readonly sequence: number;
  readonly run: "target" | "other";
  readonly kind: EventKind;
}

interface PollCase {
  readonly from: number;
  readonly snapshots: readonly (readonly EventSpec[])[];
}

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
}

function eventFromSpec(spec: EventSpec, sessionId: string): AexEvent {
  const data: Record<string, JsonValue> = spec.kind === "RUN_FINISHED"
    ? { outcome: "succeeded", costUsd: 0, providerUsage: [], checkpoint: { checkpointId: "cp-property" } }
    : spec.kind === "RUN_ERROR"
      ? {
          outcome: "failed",
          failureClass: "provider-permanent",
          failureMessage: "property failure",
          costUsd: 0,
          providerUsage: []
        }
      : { name: "property.event", value: spec.sequence };
  return {
    specversion: "1.0",
    id: `event-${spec.id}`,
    source: "runtime",
    type: spec.kind,
    subject: sessionId,
    threadId: sessionId,
    runId: spec.run === "target" ? "run-target" : "run-other",
    time: new Date(spec.sequence).toISOString(),
    sequence: spec.sequence,
    data
  };
}

function referenceTrace(testCase: PollCase): { readonly lists: number; readonly yielded: readonly string[] } {
  const seen = new Set<string>();
  const yielded: string[] = [];
  let lists = 0;
  for (const snapshot of testCase.snapshots) {
    lists += 1;
    let terminalSeen = false;
    for (const spec of snapshot) {
      const id = `event-${spec.id}`;
      if (spec.sequence >= testCase.from && !seen.has(id)) {
        seen.add(id);
        yielded.push(`${id}:${spec.sequence}`);
      }
      if (spec.run === "target" && (spec.kind === "RUN_FINISHED" || spec.kind === "RUN_ERROR")) {
        terminalSeen = true;
      }
    }
    if (terminalSeen) return { lists, yielded };
  }
  throw new Error("generated polling case did not terminate");
}

async function actualTrace(mode: StreamMode, testCase: PollCase): Promise<{
  readonly lists: number;
  readonly yielded: readonly string[];
}> {
  let lists = 0;
  const parentId = "session-property";
  const childId = "child-property";
  const fetchStub: FetchLike = async (input) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    const pathname = new URL(url).pathname;
    if (pathname === `/api/sessions/${parentId}`) {
      return jsonResponse({
        session: {
          id: parentId,
          status: "running",
          acceptsMessages: false,
          currentRun: { sessionId: parentId, runId: "run-target", turnSeq: 1, phase: "running" }
        }
      });
    }
    if (pathname === `/api/sessions/${parentId}/children`) {
      return jsonResponse({
        children: [{
          id: childId,
          parentSessionId: parentId,
          status: "idle",
          createdAt: "2026-07-21T00:00:00.000Z",
          updatedAt: "2026-07-21T00:00:01.000Z",
          lastRun: { sessionId: childId, runId: "run-target", turnSeq: 1, phase: "finished", outcome: "succeeded" }
        }]
      });
    }
    const expectedEventsPath = mode === "parent"
      ? `/api/sessions/${parentId}/events`
      : `/api/sessions/${childId}/events`;
    if (pathname === expectedEventsPath) {
      const snapshot = testCase.snapshots[lists];
      lists += 1;
      if (!snapshot) throw new Error(`unexpected event snapshot request ${lists}`);
      const sessionId = mode === "parent" ? parentId : childId;
      return jsonResponse({ events: snapshot.map((spec) => eventFromSpec(spec, sessionId)) });
    }
    throw new Error(`unexpected property request ${pathname}`);
  };
  const parent = await new Aex({
    apiKey: "property-token",
    baseUrl: "https://property.test",
    fetch: fetchStub,
    retry: false
  }).sessions.open(parentId);
  const stream = mode === "parent"
    ? parent.events.stream({ from: testCase.from, intervalMs: 0 })
    : (await parent.children())[0]!.events.stream({ from: testCase.from, intervalMs: 0 });
  const yielded: string[] = [];
  for await (const event of stream) yielded.push(`${event.id}:${event.sequence}`);
  return { lists, yielded };
}

const eventSpec = fc.record<EventSpec>({
  id: fc.integer({ min: 0, max: 4 }),
  sequence: fc.integer({ min: 0, max: 12 }),
  run: fc.constantFrom("target", "other"),
  kind: fc.constantFrom("CUSTOM", "RUN_FINISHED", "RUN_ERROR")
});

const pollCase = fc.record({
  from: fc.integer({ min: 0, max: 12 }),
  prefix: fc.array(fc.array(eventSpec, { maxLength: 5 }), { maxLength: 2 }),
  finalBefore: fc.array(eventSpec, { maxLength: 4 }),
  finalTerminal: fc.record<EventSpec>({
    id: fc.integer({ min: 0, max: 4 }),
    sequence: fc.integer({ min: 0, max: 12 }),
    run: fc.constant("target"),
    kind: fc.constantFrom("RUN_FINISHED", "RUN_ERROR")
  }),
  finalAfter: fc.array(eventSpec, { maxLength: 3 })
}).map(({ from, prefix, finalBefore, finalTerminal, finalAfter }): PollCase => ({
  from,
  snapshots: [...prefix, [...finalBefore, finalTerminal, ...finalAfter]]
}));

// bun's describe() takes no options object; this file-wide default replaces the
// former vitest describe-level { timeout: 20_000 } (single suite spans the file).
setDefaultTimeout(20_000);

describe("parent/child event polling state machine", () => {
  it("matches the bounded reference trace for generated snapshots", async () => {
    await fc.assert(
      fc.asyncProperty(fc.constantFrom<StreamMode>("parent", "child"), pollCase, async (mode, testCase) => {
        expect(await actualTrace(mode, testCase)).toEqual(referenceTrace(testCase));
      }),
      { numRuns: 40, seed: 20_260_721 }
    );
  });
});
