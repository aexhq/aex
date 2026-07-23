import { readFileSync } from "node:fs";
import { describe, expect, it } from "bun:test";

const runtimeTypes = readFileSync(new URL("../src/runtime-types.ts", import.meta.url), "utf8");
const operations = readFileSync(new URL("../src/operations.ts", import.meta.url), "utf8");

describe("public session run taxonomy source ownership", () => {
  it("derives the public phase type from the exported tuple", () => {
    expect(runtimeTypes).toMatch(/export const SESSION_RUN_PHASES\s*=\s*\[/u);
    expect(runtimeTypes).toMatch(/type SessionRunPhase\s*=\s*\(typeof SESSION_RUN_PHASES\)\[number\]/u);
  });

  it("makes the operation normalizer consume both canonical owners", () => {
    expect(operations).toMatch(/import \{ SESSION_RUN_PHASES \} from "\.\/runtime-types\.js"/u);
    expect(operations).toMatch(/SESSION_STATUSES, SESSION_TERMINAL_OUTCOMES/u);
    expect(operations).toMatch(/new Set<string>\(SESSION_RUN_PHASES\)/u);
    expect(operations).toMatch(/new Set<string>\(SESSION_TERMINAL_OUTCOMES\)/u);
    expect(operations).not.toMatch(/new Set\(\[\s*"queued"/u);
    expect(operations).not.toMatch(/new Set\(\[\s*"succeeded"/u);
  });
});
