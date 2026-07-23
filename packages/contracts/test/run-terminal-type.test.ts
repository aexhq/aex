import { readFileSync } from "node:fs";
import { describe, expect, it } from "bun:test";
import * as publicContracts from "../src/index.js";
import { hasRunTerminalType, type RunTerminalTypeEvent } from "../src/internal.js";
import { AEX_EVENT_SPECVERSION, type AexEvent, type JsonValue } from "../src/index.js";

function event(type: string, data: Readonly<Record<string, JsonValue>>): AexEvent {
  return {
    specversion: AEX_EVENT_SPECVERSION,
    id: "evt_terminal_type",
    source: "runtime",
    type,
    subject: "ses_terminal_type",
    threadId: "ses_terminal_type",
    runId: "run_terminal_type",
    time: "2026-07-21T12:00:00.000Z",
    sequence: 1,
    data
  };
}

function assertDiscriminantNarrowing(candidate: AexEvent): void {
  if (!hasRunTerminalType(candidate)) return;
  const narrowed: RunTerminalTypeEvent<AexEvent> = candidate;
  const terminalType: "RUN_FINISHED" | "RUN_ERROR" = candidate.type;
  void narrowed;
  void terminalType;
}

void assertDiscriminantNarrowing;

describe("internal run-terminal discriminant ownership", () => {
  it("recognizes terminal types without claiming malformed payloads are validated", () => {
    expect(hasRunTerminalType(event("RUN_FINISHED", { outcome: "succeeded" }))).toBe(true);
    expect(hasRunTerminalType(event("RUN_ERROR", {}))).toBe(true);
    expect(hasRunTerminalType(event("RUN_STARTED", {}))).toBe(false);
  });

  it("keeps the raw streaming guard internal and shared by the coordinator", () => {
    expect("hasRunTerminalType" in publicContracts).toBe(false);
    const streamSource = readFileSync(new URL("../src/event-stream-client.ts", import.meta.url), "utf8");
    expect(streamSource).toContain("opts.isTerminal ?? hasRunTerminalType");
    expect(streamSource.match(/event\.type\s*===\s*"RUN_FINISHED"\s*\|\|\s*event\.type\s*===\s*"RUN_ERROR"/g)).toHaveLength(1);
  });
});
