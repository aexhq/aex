import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "bun:test";
import {
  PUBLIC_RUNTIME_PARITY_SCENARIOS,
  buildExpectedParityCells,
  scenarioCellKey,
  validateParityVerdicts,
  type ScenarioVerdict
} from "../_fixtures/runtime-parity-ledger.js";

/** `apps/user-tests` — every ledger `sourceFiles` entry is relative to it. */
const appRoot = resolve(import.meta.dirname, "..", "..");

describe("public runtime-parity scenario ledger", () => {
  it("has stable unique scenario IDs and complete execution-runtime coverage", () => {
    const ids = PUBLIC_RUNTIME_PARITY_SCENARIOS.map(({ id }) => id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(ids).toEqual([...ids].sort());

    for (const scenario of PUBLIC_RUNTIME_PARITY_SCENARIOS) {
      expect(scenario.runtimes).toEqual(["container", "spot_container", "lambda"]);
      expect(scenario.layers.e2e.length).toBeGreaterThan(0);
      expect(scenario.layers.user.length).toBeGreaterThan(0);
      expect(scenario.sourceFiles.length).toBeGreaterThan(0);
    }
  });

  // A `length > 0` check let `edge-byok-secrets.user.test.ts` sit in the ledger for
  // a month after the managed-gateway pivot deleted it: the ledger claimed evidence
  // from a file that no longer existed. Assert the claim resolves to a real file.
  it("names only source files that exist on disk", () => {
    const missing: string[] = [];
    for (const scenario of PUBLIC_RUNTIME_PARITY_SCENARIOS) {
      for (const sourceFile of scenario.sourceFiles) {
        if (!existsSync(resolve(appRoot, sourceFile))) {
          missing.push(`${scenario.id} -> ${sourceFile}`);
        }
      }
    }
    expect(missing, "ledger scenarios claim evidence from files that do not exist").toEqual([]);
  });

  it("requires SDK and CLI evidence for every core customer journey", () => {
    for (const scenario of PUBLIC_RUNTIME_PARITY_SCENARIOS) {
      expect(scenario.layers.e2e, scenario.id).toContain("sdk");
      expect(scenario.layers.e2e, scenario.id).toContain("cli");
      expect(scenario.layers.user, scenario.id).toContain("sdk");
      expect(scenario.layers.user, scenario.id).toContain("cli");
    }
  });

  it("generates a deterministic, complete scenario/layer/entrypoint/runtime manifest", () => {
    const cells = buildExpectedParityCells(PUBLIC_RUNTIME_PARITY_SCENARIOS);
    expect(cells.length).toBe(PUBLIC_RUNTIME_PARITY_SCENARIOS.length * 12);
    expect(new Set(cells.map(scenarioCellKey)).size).toBe(cells.length);
    expect(cells.map(scenarioCellKey)).toEqual([...cells.map(scenarioCellKey)].sort());
  });

  it("accepts exactly one passing and cleaned verdict for every expected cell", () => {
    const expected = buildExpectedParityCells(PUBLIC_RUNTIME_PARITY_SCENARIOS);
    const verdicts: ScenarioVerdict[] = expected.map((cell) => ({
      ...cell,
      status: "passed",
      cleanup: "passed",
      evidenceDigest: "sha256:fixture"
    }));

    expect(validateParityVerdicts(expected, verdicts)).toEqual([]);
  });

  it("fails closed on missing, duplicate, extra, skipped, substituted, or dirty cells", () => {
    const expected = buildExpectedParityCells(PUBLIC_RUNTIME_PARITY_SCENARIOS).slice(0, 3);
    const [first, second, third] = expected;
    if (!first || !second || !third) throw new Error("fixture must contain three cells");

    const verdicts: ScenarioVerdict[] = [
      { ...first, status: "passed", cleanup: "passed", evidenceDigest: "sha256:first" },
      { ...first, status: "passed", cleanup: "passed", evidenceDigest: "sha256:duplicate" },
      { ...second, status: "skipped", cleanup: "passed", evidenceDigest: "sha256:skip" },
      {
        ...third,
        scenarioId: "unexpected.scenario",
        status: "passed",
        cleanup: "failed",
        evidenceDigest: "sha256:extra"
      }
    ];

    const failures = validateParityVerdicts(expected, verdicts).join("\n");
    expect(failures).toContain(`duplicate verdict: ${scenarioCellKey(first)}`);
    expect(failures).toContain(`non-passing verdict: ${scenarioCellKey(second)} status=skipped`);
    expect(failures).toContain(`missing verdict: ${scenarioCellKey(third)}`);
    expect(failures).toContain("unexpected verdict:");
    expect(failures).toContain("cleanup=failed");
  });
});
