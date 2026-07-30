import { resolve } from "node:path";
import { describe, expect, it } from "bun:test";

import { MODULE_LANES, dependencyBuildPlan, planModuleLanes, runModuleLanes } from "../cicd/run-module-checks.mjs";

const repoRoot = resolve(import.meta.dir, "..", "..");

describe("module lanes are declared by the module, not assumed by the runner", () => {
  it("runs build before typecheck, lint, and unit tests", () => {
    expect([...MODULE_LANES]).toEqual(["build", "typecheck", "lint", "test:unit"]);
  });

  it("reports the lanes a module does not declare instead of implying they passed", () => {
    const docs = planModuleLanes(repoRoot, "docs");
    expect(docs.declared.length + docs.undeclared.length).toBe(MODULE_LANES.length);
    expect(docs.undeclared).toContain("test:unit");

    const sdk = planModuleLanes(repoRoot, "sdk");
    expect(sdk.declared).toEqual(["build", "typecheck", "lint", "test:unit"]);
    expect(sdk.undeclared).toEqual([]);
  });

  it("rejects an unknown module", () => {
    expect(() => planModuleLanes(repoRoot, "ghost")).toThrow(/unknown public module/);
  });

  it("prebuilds workspace dependencies in dependency order", () => {
    expect(dependencyBuildPlan(repoRoot, "user-tests")).toEqual([{ id: "contracts", name: "@aexhq/contracts" }]);
  });
});

describe("a failing lane does not hide the lanes after it", () => {
  it("runs every declared lane even once one has failed, then fails overall", () => {
    const attempted: string[] = [];
    const outcome = runModuleLanes(repoRoot, "sdk", (_root, _name, lane) => {
      attempted.push(`${_name}:${lane}`);
      return lane === "typecheck" ? 2 : 0;
    });
    expect(attempted).toEqual([
      "@aexhq/contracts:build",
      "@aexhq/sdk:build",
      "@aexhq/sdk:typecheck",
      "@aexhq/sdk:lint",
      "@aexhq/sdk:test:unit"
    ]);
    expect(outcome.ok).toBe(false);
    expect(outcome.results.find((entry) => entry.lane === "typecheck")?.status).toBe(2);
  });

  it("passes only when every declared lane passed", () => {
    const outcome = runModuleLanes(repoRoot, "contracts", () => 0);
    expect(outcome.ok).toBe(true);
    expect(outcome.results.every((entry) => entry.status === 0)).toBe(true);
  });

  it("materializes contracts before user-test typechecking in a fresh lane", () => {
    const attempted: string[] = [];
    const outcome = runModuleLanes(repoRoot, "user-tests", (_root, name, lane) => {
      attempted.push(`${name}:${lane}`);
      return 0;
    });
    expect(attempted).toEqual(["@aexhq/contracts:build", "@aexhq/user-tests:typecheck", "@aexhq/user-tests:lint"]);
    expect(outcome.prerequisites).toEqual([{ id: "contracts", name: "@aexhq/contracts", status: 0 }]);
    expect(outcome.ok).toBe(true);
  });
});
