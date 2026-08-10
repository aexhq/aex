import { describe, expect, it } from "bun:test";
import {
  assertExecutionAllowed,
  buildAffectedPlan,
  mergeChangedPaths,
  parseArguments,
  parseNullSeparated,
  type SelectionDocument
} from "../dev/affected.js";

const selection: SelectionDocument = {
  schema: "aex.selection.v1",
  lane: "pr",
  mode: "affected",
  test: [
    {
      id: "cargo:aex-leaf",
      reason: "changed-source",
      path: "crates/aex-leaf/src/lib.rs"
    },
    {
      id: "cargo:aex-dependent",
      reason: "reverse-dependency",
      of: "cargo:aex-leaf"
    },
    {
      id: "npm:@aexhq/dashboard",
      reason: "changed-source",
      path: "apps/dashboard/app/page.tsx"
    },
    {
      id: "bundle:contract",
      reason: "reverse-dependency",
      of: "cargo:aex-leaf"
    }
  ],
  deploy: [],
  scenarios: [],
  repo_wide: false,
  unowned: [],
  changed_paths: 2,
  routing_failed: false,
  routing_failure_reason: null
};

describe("affected local development arguments", () => {
  it("defaults to a plan against local main", () => {
    expect(parseArguments([], 6)).toEqual({
      actions: [],
      base: "main",
      help: false,
      jobs: 6
    });
  });

  it("parses a deterministic action set and explicit cache", () => {
    expect(
      parseArguments(
        [
          "--run",
          "test,check,test",
          "--base",
          "HEAD~2",
          "--jobs",
          "3",
          "--target-dir",
          "../aex-target",
          "--release-tool",
          "./target/debug/aex-release-tool"
        ],
        8
      )
    ).toEqual({
      actions: ["check", "test"],
      base: "HEAD~2",
      help: false,
      jobs: 3,
      releaseTool: "./target/debug/aex-release-tool",
      targetDir: "../aex-target"
    });
  });

  it("rejects unknown actions and invalid job counts", () => {
    expect(() => parseArguments(["--run", "deploy"], 8)).toThrow("unknown action");
    expect(() => parseArguments(["--jobs", "0"], 8)).toThrow("positive integer");
  });
});

describe("affected change input", () => {
  it("preserves commas and newlines in NUL-delimited Git paths", () => {
    expect(parseNullSeparated("a,b.rs\0line\nbreak.ts\0")).toEqual([
      "a,b.rs",
      "line\nbreak.ts"
    ]);
  });

  it("normalizes, deduplicates, and sorts all diff sources", () => {
    expect(
      mergeChangedPaths(
        ["crates\\z\\src\\lib.rs", "apps/a/page.ts"],
        ["apps/a/page.ts", "crates/a/src/lib.rs"]
      )
    ).toEqual(["apps/a/page.ts", "crates/a/src/lib.rs", "crates/z/src/lib.rs"]);
  });
});

describe("affected execution plan", () => {
  it("checks the exact Cargo reverse closure and affected npm package", () => {
    const plan = buildAffectedPlan(selection, ["check"], 4);

    expect(plan.rustPackages).toEqual(["aex-dependent", "aex-leaf"]);
    expect(plan.nodePackages).toEqual(["@aexhq/dashboard"]);
    expect(plan.commands).toEqual([
      {
        args: [
          "check",
          "--locked",
          "--all-targets",
          "--jobs",
          "4",
          "-p",
          "aex-dependent",
          "-p",
          "aex-leaf"
        ],
        program: "cargo"
      },
      {
        args: [
          "run",
          "--filter",
          "@aexhq/dashboard",
          "--parallel",
          "--if-present",
          "typecheck"
        ],
        program: "bun"
      },
      {
        args: [
          "run",
          "--filter",
          "@aexhq/dashboard",
          "--parallel",
          "--if-present",
          "lint"
        ],
        program: "bun"
      }
    ]);
  });

  it("formats only changed Cargo owners, not their dependants", () => {
    const plan = buildAffectedPlan(selection, ["fmt"], 4);

    expect(plan.commands).toEqual([
      {
        args: ["fmt", "-p", "aex-leaf", "--", "--check"],
        program: "cargo"
      }
    ]);
  });

  it("runs affected nextest while naming the evidence left to full gates", () => {
    const plan = buildAffectedPlan(selection, ["test"], 2);

    expect(plan.commands[0]).toEqual(
      {
        args: [
          "nextest",
          "run",
          "--locked",
          "--profile",
          "default",
          "--build-jobs",
          "2",
          "--test-threads",
          "2",
          "-p",
          "aex-dependent",
          "-p",
          "aex-leaf",
          "--no-tests=fail"
        ],
        program: "cargo"
      }
    );
    expect(plan.excludedEvidence).toContain("doctests and no-skip inventory receipts");
    expect(plan.excludedEvidence).toContain("integration-engine tests");
    expect(plan.excludedEvidence).toContain("hosted release evidence");
  });

  it("does not lose a node-only test selection", () => {
    const nodeOnly = {
      ...selection,
      test: selection.test.filter((selected) => selected.id.startsWith("npm:"))
    };

    expect(buildAffectedPlan(nodeOnly, ["test"], 2).commands).toEqual([
      {
        args: [
          "run",
          "--filter",
          "@aexhq/dashboard",
          "--parallel",
          "--if-present",
          "test:unit"
        ],
        program: "bun"
      }
    ]);
  });

  it("fails closed when routing is degraded or has unowned paths", () => {
    expect(() =>
      buildAffectedPlan({ ...selection, routing_failed: true }, ["check"], 4)
    ).toThrow("routing failed");
    expect(() =>
      buildAffectedPlan({ ...selection, unowned: ["mystery/file.rs"] }, ["check"], 4)
    ).toThrow("unowned");
  });

  it("keeps broad Cargo validation on local main", () => {
    const broad = buildAffectedPlan({ ...selection, repo_wide: true }, ["check"], 4);

    expect(() => assertExecutionAllowed(broad, "topic/change")).toThrow("local main");
    expect(() => assertExecutionAllowed(broad, "main")).not.toThrow();
  });
});
