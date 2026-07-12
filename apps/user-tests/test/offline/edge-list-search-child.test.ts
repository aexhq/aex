import ts from "typescript";
import { describe, expect, it } from "vitest";
import {
  buildEdgeListSearchChildScript,
  EDGE_SESSION_DEBUG_BODY
} from "../_fixtures/edge-list-search-child.js";

describe("edge list/search child script generation", () => {
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
