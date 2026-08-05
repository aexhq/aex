import { describe, expect, test } from "bun:test";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { resolve } from "node:path";

const workflowsRoot = resolve(import.meta.dir, "../../.github/workflows");
const workflowPaths = readdirSync(workflowsRoot)
  .filter((name) => name.endsWith(".yml"))
  .map((name) => `.github/workflows/${name}`)
  .sort();

describe("public delivery ownership", () => {
  test("the public repository has no hosted release entry point", () => {
    expect(existsSync(resolve(workflowsRoot, "release.yml"))).toBeFalse();
    expect(existsSync(resolve(workflowsRoot, "_release-engine.yml"))).toBeFalse();
  });

  test("public workflows neither request private bindings nor target hosted planes", () => {
    for (const path of workflowPaths) {
      const source = readFileSync(resolve(import.meta.dir, "../..", path), "utf8");
      expect(source).not.toMatch(/\bbinding_ref\b/);
      expect(source).not.toMatch(/\brepository:\s*aexhq\/platform\b/);
      expect(source).not.toMatch(/\benvironment:\s*aex-(?:dev|prd)\b/);
      if (path === ".github/workflows/model-catalog-publish.yml") {
        expect(source).toContain("environment: aex-model-catalog-publisher");
        expect(source.match(/aws-actions\/configure-aws-credentials/g)).toHaveLength(1);
      } else {
        expect(source).not.toMatch(/aws-actions\/configure-aws-credentials/);
      }
    }
  });

  test("public assurance contains only repository-owned suites", () => {
    const source = readFileSync(resolve(workflowsRoot, "assurance.yml"), "utf8");
    const workflow = Bun.YAML.parse(source) as {
      readonly on: Readonly<Record<string, unknown>>;
      readonly jobs: Readonly<Record<string, unknown>>;
    };
    const dispatch = workflow.on.workflow_dispatch as {
      readonly inputs?: Readonly<Record<string, { readonly options?: readonly string[] }>>;
    };
    const options = dispatch.inputs?.suite?.options ?? [];

    expect(options).toEqual(["full-graph", "supply-chain", "deep-risk", "cold-rebuild"]);
    expect(workflow.jobs).not.toHaveProperty("plane");
  });
});
