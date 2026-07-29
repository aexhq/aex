import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { afterAll, describe, expect, it } from "bun:test";

import {
  classifyPath,
  dependentClosure,
  fallbackResult,
  moduleMatrix,
  parseArgs,
  readModuleGraph,
  renderOutputs,
  route,
  routeChanges
} from "../cicd/public-module-graph.mjs";

const repoRoot = resolve(import.meta.dir, "..", "..");
const temporaryRoots: string[] = [];

afterAll(() => {
  for (const root of temporaryRoots) rmSync(root, { recursive: true, force: true });
});

function fixtureRepo(packages: Record<string, Record<string, unknown>>, root: Record<string, unknown> = {}): string {
  const dir = mkdtempSync(resolve(tmpdir(), "aex-module-graph-"));
  temporaryRoots.push(dir);
  writeFileSync(
    resolve(dir, "package.json"),
    JSON.stringify({ name: "@fixture/root", private: true, workspaces: ["packages/*"], ...root })
  );
  for (const [name, manifest] of Object.entries(packages)) {
    const packageDir = resolve(dir, "packages", name);
    mkdirSync(packageDir, { recursive: true });
    writeFileSync(resolve(packageDir, "package.json"), JSON.stringify(manifest));
  }
  return dir;
}

describe("the public module graph is derived from the workspace, not restated", () => {
  it("reads every workspace package with its publishability and version", () => {
    const graph = readModuleGraph(repoRoot);
    const ids = graph.modules.map((node) => node.id);
    expect(ids).toContain("sdk");
    expect(ids).toContain("contracts");
    expect(ids).toContain("cli");

    for (const node of graph.modules) {
      expect(node.name, `${node.id} must expose its package name`).toMatch(/^@?[a-z0-9@/._-]+$/);
      expect(node.version, `${node.id} must expose a version`).toMatch(/^\d+\.\d+\.\d+/);
    }
    // Only genuinely publishable packages may ever reach the publish matrix.
    expect(graph.byId.get("sdk")!.publishable).toBe(true);
    expect(graph.byId.get("contracts")!.publishable).toBe(true);
    expect(graph.byId.get("cli")!.publishable).toBe(true);
    expect(graph.byId.get("docs")!.publishable).toBe(false);
    expect(graph.byId.get("user-tests")!.publishable).toBe(false);
  });

  it("counts a devDependency edge, because the SDK embeds contracts at build time", () => {
    const graph = readModuleGraph(repoRoot);
    expect(graph.byId.get("sdk")!.dependsOn).toContain("contracts");
    expect(graph.byId.get("cli")!.dependsOn).toContain("contracts");
  });

  it("knows the SDK embeds the CLI, because it republishes that bundle as `aex`", () => {
    // `packages/sdk/scripts/bundle-cli.mjs` copies packages/cli/dist/cli.mjs into
    // the SDK's own dist and `bin.aex` points at the copy. Both packages therefore
    // ship the SAME executable, and `packages/sdk/test/unit/bin-bundle.test.ts`
    // asserts they are byte-identical per commit.
    //
    // Without this edge a CLI-only change published a new @aexhq/cli and NO new
    // @aexhq/sdk, so the two `aex` binaries skewed on the registry from that push
    // onward — which is exactly the drift measured between the published
    // @aexhq/cli@0.25.2 and @aexhq/sdk@0.43.0. The edge is what makes "the release
    // loop publishes both, so they cannot skew" a true statement rather than an
    // assumed one.
    const graph = readModuleGraph(repoRoot);
    expect(graph.byId.get("sdk")!.dependsOn).toContain("cli");
  });

  it("resolves a bare specifier through the root overrides map", () => {
    // `apps/docs` depends on `aex`, which the root `overrides` maps to
    // `./packages/sdk`. Without that resolution the SDK loses a dependent and a
    // docs-breaking SDK change routes to nothing.
    const graph = readModuleGraph(repoRoot);
    expect(graph.byId.get("docs")!.dependsOn).toContain("sdk");
  });

  it("refuses a workspace dependency it cannot resolve", () => {
    const dir = fixtureRepo({
      alpha: { name: "@fixture/alpha", version: "1.0.0", dependencies: { "@fixture/ghost": "workspace:*" } }
    });
    expect(() => readModuleGraph(dir)).toThrow(/unresolved workspace package @fixture\/ghost/);
  });

  it("refuses an unsupported workspaces glob rather than silently dropping modules", () => {
    const dir = fixtureRepo({ alpha: { name: "@fixture/alpha", version: "1.0.0" } }, { workspaces: ["packages/**"] });
    expect(() => readModuleGraph(dir)).toThrow(/unsupported workspaces glob/);
  });

  it("refuses a dependency cycle", () => {
    const dir = fixtureRepo({
      alpha: { name: "@fixture/alpha", version: "1.0.0", dependencies: { "@fixture/beta": "workspace:*" } },
      beta: { name: "@fixture/beta", version: "1.0.0", dependencies: { "@fixture/alpha": "workspace:*" } }
    });
    expect(() => readModuleGraph(dir)).toThrow(/dependency cycle/);
  });
});

describe("a change routes its dependent closure, not just the changed module", () => {
  it("expands upstream changes to every dependent", () => {
    const graph = readModuleGraph(repoRoot);
    const routed = routeChanges(graph, ["packages/contracts/src/index.ts"]);
    expect(routed.affected).toEqual(["contracts"]);
    expect(routed.closure).toContain("sdk");
    expect(routed.closure).toContain("cli");
    expect(routed.repoWide).toBe(false);
    // The private consumers are the publishable subset, in a stable order.
    expect(routed.publish).toEqual(["cli", "contracts", "sdk"]);
  });

  it("does not expand a leaf change upstream, but does reach its dependents", () => {
    const graph = readModuleGraph(repoRoot);
    const routed = routeChanges(graph, ["packages/cli/src/index.ts"]);
    // Upstream is untouched: the CLI's own change cannot alter contracts.
    expect(routed.closure).not.toContain("contracts");
    // Downstream is not: the SDK republishes the CLI bundle as its `aex` bin, so
    // a CLI change is a change to the published SDK artifact too.
    expect(routed.publish).toEqual(["cli", "sdk"]);
  });

  it("treats a path no module owns as repo-wide rather than as no impact", () => {
    const graph = readModuleGraph(repoRoot);
    for (const path of ["package.json", "bun.lock", ".github/workflows/ci.yml", "scripts/cicd/module-canary.mjs"]) {
      const routed = routeChanges(graph, [path]);
      expect(routed.repoWide, `${path} must route repo-wide`).toBe(true);
      expect(routed.affected.length, path).toBe(graph.modules.length);
    }
  });

  it("classifies a module path to its owner and everything else to none", () => {
    const graph = readModuleGraph(repoRoot);
    expect(classifyPath(graph, "packages/sdk/src/index.ts")).toBe("sdk");
    expect(classifyPath(graph, "apps/docs/next.config.ts")).toBe("docs");
    expect(classifyPath(graph, "references/rules.md")).toBeNull();
    // A sibling directory sharing a prefix must not be captured.
    expect(classifyPath(graph, "packages/sdk-extra/index.ts")).toBeNull();
  });

  it("rejects an unknown module id instead of silently dropping it", () => {
    const graph = readModuleGraph(repoRoot);
    expect(() => dependentClosure(graph, ["not-a-module"])).toThrow(/unknown module/);
  });
});

describe("verification fails open and publication fails closed", () => {
  it("routes everything and publishes nothing when routing fails", () => {
    const result = fallbackResult(repoRoot, "git diff exploded");
    expect(result.routingFailed).toBe(true);
    expect(result.affected.length).toBeGreaterThan(0);
    expect(result.closure.length).toBe(result.affected.length);
    // The irreversible operation is withheld; the repeatable one is widened.
    expect(result.publish).toEqual([]);
    expect(result.publishMatrix).toEqual({ include: [] });
  });

  it("always emits every routing output, including on the failure path", () => {
    const keys = (rendered: string) =>
      rendered.split("\n").filter(Boolean).map((line) => line.slice(0, line.indexOf("=")));
    const healthy = keys(renderOutputs(route({ repoRoot, paths: ["packages/sdk/src/index.ts"] })));
    const degraded = keys(renderOutputs(fallbackResult(repoRoot, "boom")));
    expect(degraded).toEqual(healthy);
    // A detector that omits its outputs routes every `if:` guard to false, which
    // reads as "nothing needed to run" rather than "the detector broke".
    expect(healthy).toEqual([
      "routing_failed",
      "repo_wide",
      "affected",
      "closure",
      "publish",
      "checks_matrix",
      "publish_matrix",
      "has_publish",
      "json"
    ]);
  });

  it("renders a matrix GitHub Actions can consume directly", () => {
    const graph = readModuleGraph(repoRoot);
    const matrix = moduleMatrix(graph, ["sdk"]);
    expect(matrix).toEqual({
      include: [{ id: "sdk", name: "@aexhq/sdk", dir: "packages/sdk", version: graph.byId.get("sdk")!.version }]
    });
  });
});

describe("router argument parsing", () => {
  it("requires commit shas when no explicit path list is given", () => {
    expect(() => parseArgs([])).toThrow(/--base must be a commit sha/);
    expect(() => parseArgs(["--base", "abc1234"])).toThrow(/--head must be a commit sha/);
    expect(() => parseArgs(["--base", "main", "--head", "abc1234"])).toThrow(/--base must be a commit sha/);
  });

  it("accepts a comma or newline separated path list", () => {
    expect(parseArgs(["--paths", "a.ts,b.ts"]).paths).toEqual(["a.ts", "b.ts"]);
    expect(parseArgs(["--paths", "a.ts\nb.ts"]).paths).toEqual(["a.ts", "b.ts"]);
  });

  it("rejects an unknown flag", () => {
    expect(() => parseArgs(["--nope", "1"])).toThrow(/unknown argument/);
  });
});
