import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

describe("subagent-runtime contract entrypoint", () => {
  it("stays isolated from the event-stream client", () => {
    const source = readFileSync(resolve(import.meta.dirname, "../src/subagent-runtime.ts"), "utf8");
    expect(source).not.toContain("event-stream-client");
    expect(source).not.toContain('from "./index.js"');
  });
});
