import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "bun:test";
import {
  AEX_EVENT_SPECVERSION,
  type AexEvent,
  type JsonValue
} from "@aexhq/contracts";
import {
  isSessionRunTerminalEvent,
  latestRunTerminalEvent,
  terminalSessionStatusFromEvents
} from "../../src/event-projection.js";

const here = dirname(fileURLToPath(import.meta.url));
const sourceRoot = resolve(here, "..", "..", "src");

function event<T extends string>(
  sequence: number,
  type: T,
  data: Readonly<Record<string, JsonValue>>,
  runId = "run_target"
): AexEvent & { readonly type: T } {
  return {
    specversion: AEX_EVENT_SPECVERSION,
    id: `evt_${sequence}`,
    source: "runtime",
    type,
    subject: "ses_terminal",
    threadId: "ses_terminal",
    runId,
    time: new Date(sequence * 1_000).toISOString(),
    sequence,
    data
  };
}

function assertRunTerminalTypeNarrowing(candidate: AexEvent): void {
  if (!isSessionRunTerminalEvent(candidate, "run_target")) return;
  const terminalType: "RUN_FINISHED" | "RUN_ERROR" = candidate.type;
  void terminalType;
}

void assertRunTerminalTypeNarrowing;

describe("SDK run-terminal ownership", () => {
  it("combines run identity with the contracts-owned terminal discriminant guard", () => {
    const finished = event(1, "RUN_FINISHED", { outcome: "succeeded" });
    const failed = event(2, "RUN_ERROR", {
      outcome: "failed",
      failureClass: "provider-permanent",
      failureMessage: "provider rejected the request"
    });
    const otherRun = event(3, "RUN_FINISHED", { outcome: "succeeded" }, "run_other");
    const malformed = event(4, "RUN_FINISHED", {});

    expect(isSessionRunTerminalEvent(finished, "run_target")).toBe(true);
    expect(isSessionRunTerminalEvent(failed, "run_target")).toBe(true);
    expect(isSessionRunTerminalEvent(otherRun, "run_target")).toBe(false);
    expect(isSessionRunTerminalEvent(malformed, "run_target")).toBe(true);
    expect(() => terminalSessionStatusFromEvents([malformed], "run_target")).toThrow(
      "RUN terminal is missing a valid explicit outcome"
    );
  });

  it("finds the newest matching terminal without copying or mutating input", () => {
    const older = event(1, "RUN_FINISHED", { outcome: "succeeded" });
    const nonTerminal = event(2, "TEXT_MESSAGE_CONTENT", { text: "still working" });
    const newest = event(3, "RUN_ERROR", {
      outcome: "failed",
      failureClass: "provider-permanent",
      failureMessage: "failed"
    });
    const otherRun = event(4, "RUN_FINISHED", { outcome: "succeeded" }, "run_other");
    const malformed = event(5, "RUN_ERROR", { outcome: "failed" });
    const events = [older, nonTerminal, newest, otherRun] as const;
    const before = [...events] as const;

    expect(latestRunTerminalEvent(events, "run_target")).toBe(newest);
    expect(latestRunTerminalEvent([...events, malformed], "run_target")).toBe(malformed);
    expect(events).toEqual(before);
    expect(latestRunTerminalEvent(events, "run_missing")).toBeUndefined();
  });

  it("keeps contracts as the terminal-kind owner and one reverse lookup in the SDK", () => {
    const projection = readFileSync(resolve(sourceRoot, "event-projection.ts"), "utf8");
    const client = readFileSync(resolve(sourceRoot, "client.ts"), "utf8");
    const copiedTerminalMembership = /\.type\s*===\s*["']RUN_FINISHED["']\s*\|\|\s*\w+\.type\s*===\s*["']RUN_ERROR["']/;

    expect(projection).not.toMatch(copiedTerminalMembership);
    expect(client).not.toMatch(copiedTerminalMembership);
    expect(projection).not.toContain("[...events].reverse()");
    expect(projection).toContain("hasRunTerminalType(event)");
    expect(projection).toMatch(
      /function latestRunTerminalEvent[\s\S]*for \(let i = events\.length - 1; i >= 0; i--\)/
    );
    expect(projection.match(/latestRunTerminalEvent\(events, runId\)/g)).toHaveLength(4);
  });
});
