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
    // The invariant is that the normalizer imports the phase tuple FROM its
    // canonical owner — not that the import statement names nothing else.
    // `runtime-types.js` also owns `RUNTIME_CAPABILITY_NAMES`, and pinning the
    // exact import form made this fail on a combined import that satisfies the
    // rule perfectly well.
    expect(operations).toMatch(/import \{[^}]*\bSESSION_RUN_PHASES\b[^}]*\} from "\.\/runtime-types\.js"/u);
    expect(operations).toMatch(/SESSION_STATUSES, SESSION_TERMINAL_OUTCOMES/u);
    expect(operations).toMatch(/new Set<string>\(SESSION_RUN_PHASES\)/u);
    expect(operations).toMatch(/new Set<string>\(SESSION_TERMINAL_OUTCOMES\)/u);
    expect(operations).not.toMatch(/new Set\(\[\s*"queued"/u);
    expect(operations).not.toMatch(/new Set\(\[\s*"succeeded"/u);
  });
});
