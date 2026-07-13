import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import ts from "typescript";
import { describe, expect, it } from "vitest";
import {
  buildEdgeListSearchChildScript,
  EDGE_SESSION_DEBUG_BODY
} from "../_fixtures/edge-list-search-child.js";

describe("edge list/search child script generation", () => {
  it("isolates scenario declarations from fixture helper names", () => {
    const script = buildEdgeListSearchChildScript(`
      const serialized = JSON.stringify({ ok: true });
      printSafe({ serialized });
    `);
    const dir = mkdtempSync(join(tmpdir(), "aex-edge-list-search-child-"));
    const path = join(dir, "edge-finish-consistency.mjs");
    try {
      writeFileSync(path, script);
      const checked = spawnSync(process.execPath, ["--check", path], { encoding: "utf8" });
      expect(checked.stderr).toBe("");
      expect(checked.status).toBe(0);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("emits syntactically valid JavaScript for the debug probe", () => {
    const script = buildEdgeListSearchChildScript(EDGE_SESSION_DEBUG_BODY);
    const compiled = ts.transpileModule(script, {
      fileName: "edge-session-debug.mjs",
      compilerOptions: {
        module: ts.ModuleKind.ESNext,
        target: ts.ScriptTarget.ESNext
      },
      reportDiagnostics: true
    });

    expect(compiled.diagnostics ?? []).toEqual([]);
    expect(script).toContain('lines.join("\\n")');
  });
});
