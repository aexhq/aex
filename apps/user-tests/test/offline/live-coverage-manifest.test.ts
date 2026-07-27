/**
 * Guards the hand-picked release matrix in `scripts/shard-files.mjs`.
 *
 * The manifest is the reason a dev deploy fans out over 26 jobs instead of ~90,
 * so the ways it can rot all need to fail here rather than in a deploy:
 *   - a NEW live file with no tier would silently inherit the cheapest fan-out;
 *   - a DELETED file left classified would claim coverage that no longer exists
 *     (exactly how a retired BYOK suite sat in the parity ledger for a month);
 *   - a runtime-spotcheck/-matrix file demoted by accident would quietly stop
 *     being proven on lambda;
 *   - the parity-verdict owners could drift from the manifest's entry points.
 */
import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import {
  COVERAGE_TIERS,
  LIVE_TEST_COVERAGE,
  ON_DEMAND_FILES,
  assertCoverageManifest,
  buildFileMatrix,
  collectAllLiveFiles,
  collectTestFiles,
  coverageFor,
  filesInTier
} from "../../scripts/shard-files.mjs";
import type { RuntimeCoverage } from "../../scripts/shard-files.mjs";
import { PARITY_SCENARIO_OWNERSHIP } from "../../scripts/runtime-parity-verdicts.mjs";

/** The declared arms per plane, mirroring the platform scenario ledger. */
const DEV = {
  fullCoverage: ["lambda", "spot_container"]
} as const satisfies RuntimeCoverage;
const PRD = {
  fullCoverage: ["spot_container"]
} as const satisfies RuntimeCoverage;

const RUNTIME_BINDING_SOURCES: Readonly<Record<string, string>> = {
  "test/live/edge-cli.user.test.ts": "test/live/edge-cli.user.test.ts",
  "test/live/live-sdk-event-stream.test.ts": "test/live/live-sdk-event-stream.test.ts",
  "test/live/live-sdk-comprehensive.test.ts": "test/live/live-sdk-comprehensive.test.ts",
  "test/live/edge-event-stream.user.test.ts": "test/live/edge-event-stream.user.test.ts",
  "test/live/edge-instructions-files.user.test.ts": "test/live/edge-instructions-files.user.test.ts",
  "test/live/edge-files.user.test.ts": "test/live/edge-files.user.test.ts",
  "test/live/edge-skills-tools.user.test.ts": "test/live/edge-skills-tools.user.test.ts",
  "test/live/config-envvars.user.test.ts": "test/live/_sdk.ts",
  "test/live/config-packages.user.test.ts": "test/live/_sdk.ts",
  "test/live/edge-chat-multiturn.user.test.ts": "test/_fixtures/edge-chat-session.ts",
  "test/live/edge-chat-suspend.user.test.ts": "test/_fixtures/edge-chat-session.ts"
};

describe("live-test coverage manifest", () => {
  it("classifies every live file on disk, and classifies nothing that is gone", () => {
    const onDisk = collectAllLiveFiles();
    expect(() => assertCoverageManifest(onDisk)).not.toThrow();
    expect(Object.keys(LIVE_TEST_COVERAGE).sort()).toEqual([...onDisk].sort());
    expect(() => assertCoverageManifest([...onDisk, "test/live/unclassified.test.ts"])).toThrow(
      /has no LIVE_TEST_COVERAGE entry/
    );
    expect(() => assertCoverageManifest(onDisk.slice(1))).toThrow(/does not exist on disk/);
  });

  it("keeps every tier non-empty so a fan-out cannot collapse unnoticed", () => {
    const sweep = collectTestFiles();
    for (const tier of COVERAGE_TIERS) {
      const files = tier === "on-demand" ? ON_DEMAND_FILES : filesInTier(tier, sweep);
      expect(files.length, `tier ${tier} is empty`).toBeGreaterThan(0);
    }
  });

  it("proves BOTH public entry points on every runtime arm", () => {
    // A matrix that only ever drove the SDK would leave the shipped `aex` binary
    // unproven on lambda. The spotcheck tier is where that is guaranteed.
    const spotcheck = filesInTier("runtime-spotcheck", collectTestFiles());
    const entryPoints = new Set(spotcheck.map((file) => coverageFor(file).entryPoint));
    expect([...entryPoints].sort()).toEqual(["cli", "sdk"]);
  });

  it("binds every runtime-sensitive file to the selected arm and checks returned identity", () => {
    const sensitive = [
      ...filesInTier("runtime-spotcheck", collectTestFiles()),
      ...filesInTier("runtime-matrix", collectTestFiles())
    ].sort();
    expect(Object.keys(RUNTIME_BINDING_SOURCES).sort()).toEqual(sensitive);
    for (const file of sensitive) {
      const owner = RUNTIME_BINDING_SOURCES[file]!;
      const source = readFileSync(resolve(import.meta.dir, "..", "..", owner), "utf8");
      expect(source, `${file} runtime binding owner ${owner}`).toContain("requireLiveRuntimeKind");
      expect(source, `${file} does not submit runtime.kind through ${owner}`).toMatch(
        /runtime:\s*\{\s*kind:|["']--runtime["']/
      );
      expect(source, `${file} does not verify returned runtime identity through ${owner}`).toMatch(
        /runtime identity mismatch|expect\(result\.runtime\)|observedRuntime/
      );
    }
  });

  it("excludes the on-demand tier from the swept matrix", () => {
    const sweep = collectTestFiles();
    for (const file of ON_DEMAND_FILES) expect(sweep).not.toContain(file);
    for (const entry of buildFileMatrix(sweep, DEV)) {
      for (const file of entry.files) expect(ON_DEMAND_FILES).not.toContain(file);
    }
  });

  it("fans runtime-sensitive tiers over full kinds and agnostic files once", () => {
    const sweep = collectTestFiles();
    const dev = buildFileMatrix(sweep, DEV);
    const kindsFor = (file: string) =>
      dev.filter((entry) => entry.files.includes(file)).map((entry) => entry.runtimeKind).sort();

    for (const file of filesInTier("runtime-spotcheck", sweep)) {
      expect(kindsFor(file), file).toEqual(["lambda", "spot_container"]);
    }
    for (const file of filesInTier("runtime-matrix", sweep)) {
      expect(kindsFor(file), file).toEqual(["lambda", "spot_container"]);
    }
    for (const file of filesInTier("runtime-agnostic", sweep)) {
      // Exactly one arm, and it is the plane's PRIMARY (shipped) container kind —
      // never the lambda arm, which is dev-only and absent on prd.
      expect(kindsFor(file), file).toEqual(["spot_container"]);
    }
  });

  it("covers every swept file exactly once per plane", () => {
    const sweep = collectTestFiles();
    for (const [label, coverage] of [["dev", DEV], ["prd", PRD]] as const) {
      const seen = new Map<string, number>();
      for (const entry of buildFileMatrix(sweep, coverage)) {
        for (const file of entry.files) seen.set(file, (seen.get(file) ?? 0) + 1);
      }
      for (const file of sweep) {
        expect(seen.get(file) ?? 0, `${label}: ${file} is not in the matrix`).toBeGreaterThan(0);
      }
      expect([...seen.keys()].sort(), `${label}: matrix covers a file outside the sweep`).toEqual(
        [...sweep].sort()
      );
    }
  });

  it("stays materially smaller than a full per-file fan-out", () => {
    // The regression this manifest exists to prevent: someone re-tiers enough
    // files to runtime-matrix that the deploy matrix drifts back toward one job
    // per file per arm. Assert the saving, not just the mechanism.
    const sweep = collectTestFiles();
    const naive = sweep.length * DEV.fullCoverage.length;
    expect(buildFileMatrix(sweep, DEV).length).toBeLessThan(naive / 2);
  });

  it("shards the agnostic tier into bins, not one job per file", () => {
    const sweep = collectTestFiles();
    const agnosticEntries = buildFileMatrix(sweep, DEV).filter(
      (entry) => entry.tier === "runtime-agnostic"
    );
    const agnosticFiles = filesInTier("runtime-agnostic", sweep);
    expect(agnosticEntries.length).toBeLessThan(agnosticFiles.length);
    expect(agnosticEntries.flatMap((entry) => entry.files).sort()).toEqual([...agnosticFiles].sort());
  });

  it("agrees with the parity-verdict owners on layer and entry point", () => {
    for (const [file, owner] of Object.entries(PARITY_SCENARIO_OWNERSHIP)) {
      const entry = coverageFor(file);
      expect(entry.entryPoint, `${file} entry point`).toBe(owner.entryPoint);
      // Only a spotcheck file sees every runtime arm, so only a spotcheck file
      // can honestly emit a parity verdict for the container cell.
      expect(entry.tier, `${file} owns parity cells but is not a spotcheck file`).toBe(
        "runtime-spotcheck"
      );
    }
  });
});
