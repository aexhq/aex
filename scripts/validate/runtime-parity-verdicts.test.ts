import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
  emitVerdicts,
  parityCellsForFile,
  validateVerdictDirectory
} from "../../apps/user-tests/scripts/runtime-parity-verdicts.mjs";

const roots: string[] = [];
const candidateIdentity = `sha256:${"b".repeat(64)}`;

afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function fixture() {
  const root = mkdtempSync(join(tmpdir(), "aex-parity-verdict-"));
  roots.push(root);
  const reportPath = join(root, "report.json");
  const verdictDirectory = join(root, "verdicts");
  mkdirSync(verdictDirectory);
  const outputPath = join(verdictDirectory, "verdict.json");
  writeFileSync(reportPath, JSON.stringify({ success: true }));
  const cells = parityCellsForFile("test/live/edge-cli.user.test.ts", "lambda");
  const matrix = [{ file: "test/live/edge-cli.user.test.ts", runtimeKind: "lambda", parityCells: cells }];
  return { root, verdictDirectory, reportPath, outputPath, cells, matrix };
}

describe("runtime parity workflow verdicts", () => {
  it("emits one evidence-bound verdict for each registered cell and closes exactly", () => {
    const value = fixture();
    const verdicts = emitVerdicts({
      cells: value.cells,
      stepOutcome: "success",
      candidateIdentity,
      reportPath: value.reportPath,
      outputPath: value.outputPath
    });

    expect(verdicts).toHaveLength(3);
    expect(verdicts.every((verdict) => verdict.status === "passed" && verdict.cleanup === "passed")).toBe(true);
    expect(verdicts.every((verdict) => /^sha256:[0-9a-f]{64}$/.test(verdict.evidenceDigest))).toBe(true);
    expect(JSON.parse(readFileSync(value.outputPath, "utf8"))).toEqual(verdicts);
    expect(validateVerdictDirectory({
      matrix: value.matrix,
      verdictDirectory: value.verdictDirectory,
      candidateIdentity
    })).toEqual({ expected: 3, actual: 3 });
  });

  it("fails closed on failed execution, missing cells, duplicates, and candidate substitution", () => {
    const value = fixture();
    emitVerdicts({
      cells: value.cells,
      stepOutcome: "failure",
      candidateIdentity,
      reportPath: value.reportPath,
      outputPath: value.outputPath
    });
    expect(() => validateVerdictDirectory({
      matrix: value.matrix,
      verdictDirectory: value.verdictDirectory,
      candidateIdentity
    })).toThrow(/non-passing verdict/);

    writeFileSync(value.outputPath, "[]\n");
    expect(() => validateVerdictDirectory({
      matrix: value.matrix,
      verdictDirectory: value.verdictDirectory,
      candidateIdentity
    })).toThrow(/missing verdict/);

    const passed = emitVerdicts({
      cells: value.cells,
      stepOutcome: "success",
      candidateIdentity,
      reportPath: value.reportPath,
      outputPath: value.outputPath
    });
    writeFileSync(join(value.verdictDirectory, "duplicate.json"), JSON.stringify(passed));
    expect(() => validateVerdictDirectory({
      matrix: value.matrix,
      verdictDirectory: value.verdictDirectory,
      candidateIdentity: `sha256:${"c".repeat(64)}`
    })).toThrow(/candidate identity mismatch|duplicate verdict/);
  });
});
