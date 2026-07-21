import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const source = readFileSync(resolve(import.meta.dirname, "../../src/event-projection.ts"), "utf8");

describe("event projection guard ownership", () => {
  it("uses canonical typed guards for known carrier payloads", () => {
    for (const guard of [
      "isCustom",
      "isRunError",
      "isTextMessage",
      "isToolCallResult",
      "isToolCallStart"
    ]) {
      expect(source).toMatch(new RegExp(`\\b${guard}\\(`));
    }

    expect(source).not.toMatch(/event\.type\s*[!=]==?\s*"(?:TEXT_MESSAGE_CONTENT|TOOL_CALL_START|TOOL_CALL_RESULT|CUSTOM)"/);
    expect(source).not.toContain("asRecord(event.data)");
  });
});
