import { existsSync, readdirSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";
import { describe, expect, it } from "vitest";
import {
  jobNeeds,
  readRepoFile,
  readWorkflow,
  workflowJob,
  workflowStep
} from "./workflow-test-helpers.js";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

function retryProperties(path: string): readonly string[] {
  const source = ts.createSourceFile(path, readRepoFile(path), ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  const found: string[] = [];
  const visit = (node: ts.Node): void => {
    if (
      (ts.isPropertyAssignment(node) || ts.isShorthandPropertyAssignment(node)) &&
      node.name.getText(source).replace(/["']/g, "") === "retry"
    ) {
      found.push(`${source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1}`);
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  return found;
}

function templateSource(node: ts.TemplateLiteral): string {
  if (ts.isNoSubstitutionTemplateLiteral(node)) return node.text;
  return node.head.text + node.templateSpans.map((span) => `undefined${span.literal.text}`).join("");
}

function suppressedSessionReads(path: string): readonly string[] {
  const source = ts.createSourceFile(path, readRepoFile(path), ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  const findings: string[] = [];
  const inspect = (text: string, label: string): void => {
    const embedded = ts.createSourceFile(label, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
    const visitEmbedded = (node: ts.Node): void => {
      if (ts.isCatchClause(node) && ts.isTryStatement(node.parent)) {
        const guarded = node.parent.tryBlock.getText(embedded);
        if (/(?:\.sessions\.(?:open|get|list)|\.(?:events|files|messages)\.list)\s*\(/.test(guarded)) {
          findings.push(label);
        }
      }
      ts.forEachChild(node, visitEmbedded);
    };
    visitEmbedded(embedded);
  };
  const visit = (node: ts.Node): void => {
    if (ts.isTemplateExpression(node) || ts.isNoSubstitutionTemplateLiteral(node)) {
      const line = source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1;
      inspect(templateSource(node), `${path}:${line}`);
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  return findings;
}

describe("live scenario reliability", () => {
  it("does not configure Vitest to rerun live scenarios", () => {
    const configDir = resolve(repoRoot, "apps/user-tests");
    for (const name of readdirSync(configDir).filter((entry) => /^vitest\..*\.config\.ts$/.test(entry))) {
      const path = `apps/user-tests/${name}`;
      expect(retryProperties(path), path).toEqual([]);
    }
  });

  it("does not keep the removed whole-scenario transport retry fixture", () => {
    expect(existsSync(resolve(repoRoot, "apps/user-tests/test/_fixtures/pre-create-transport.ts"))).toBe(false);
    expect(existsSync(resolve(repoRoot, "apps/user-tests/test/_fixtures/pre-create-transport.test.ts"))).toBe(false);
  });

  it("does not suppress post-finish session reads", () => {
    for (const path of [
      "apps/user-tests/test/live/edge-byok-secrets.user.test.ts",
      "apps/user-tests/test/live/edge-skills-tools.user.test.ts",
      "apps/user-tests/test/live/edge-instructions-files.user.test.ts",
      "apps/user-tests/test/live/edge-mcp-egress.user.test.ts",
      "apps/user-tests/test/live/edge-lineage-observability.user.test.ts"
    ]) {
      expect(suppressedSessionReads(path), path).toEqual([]);
    }
  });

  it("keeps file-level dynamic fanout at one worker per job", () => {
    const workflow = readWorkflow(".github/workflows/live-user-tests.yml");
    const job = workflowJob(workflow, "live-user-tests");
    const step = workflowStep(job, "Live user tests");

    expect(jobNeeds(job)).toEqual(expect.arrayContaining(["prepare-artifact", "live-user-tests-preflight"]));
    expect(job.strategy?.matrix?.include).toBe("${{ fromJSON(needs.prepare-artifact.outputs.test_matrix) }}");
    expect(job.strategy?.["max-parallel"]).toBeUndefined();
    expect(step.env?.AEX_USER_TEST_MAX_WORKERS).toBe(1);
    expect(step.env?.TEST_FILE).toBe("${{ matrix.file }}");
    expect(step.run).toContain('test:user:files -- "$TEST_FILE"');
    expect(step.run).not.toContain("--shard");
  });
});
