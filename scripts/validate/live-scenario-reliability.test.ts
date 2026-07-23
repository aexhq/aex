import { execFileSync } from "node:child_process";
import { readdirSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import ts from "typescript";
import { describe, expect, it } from "vitest";
import {
  jobNeeds,
  readRepoFile,
  readWorkflow,
  workflowJob,
  workflowStepRunning
} from "./workflow-test-helpers.js";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

function liveScenarioFiles(): readonly string[] {
  const root = resolve(repoRoot, "apps/user-tests/test/live");
  const files: string[] = [];
  const visit = (dir: string): void => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) visit(path);
      else if (entry.isFile() && entry.name.endsWith(".ts")) {
        files.push(relative(repoRoot, path).replaceAll("\\", "/"));
      }
    }
  };
  visit(root);
  return files.sort();
}

function templateSource(node: ts.TemplateLiteral): string {
  if (ts.isNoSubstitutionTemplateLiteral(node)) return node.text;
  return node.head.text + node.templateSpans.map((span) => `undefined${span.literal.text}`).join("");
}

function isNonzeroNumericLiteral(node: ts.Expression | undefined): boolean {
  return node !== undefined && ts.isNumericLiteral(node) && Number(node.text) !== 0;
}

function catchFailsChildProcess(node: ts.CatchClause, source: ts.SourceFile): boolean {
  let fails = false;
  const visit = (candidate: ts.Node): void => {
    if (
      ts.isBinaryExpression(candidate) &&
      candidate.operatorToken.kind === ts.SyntaxKind.EqualsToken &&
      candidate.left.getText(source) === "process.exitCode" &&
      isNonzeroNumericLiteral(candidate.right)
    ) {
      fails = true;
    }
    if (
      ts.isCallExpression(candidate) &&
      candidate.expression.getText(source) === "process.exit" &&
      isNonzeroNumericLiteral(candidate.arguments[0])
    ) {
      fails = true;
    }
    ts.forEachChild(candidate, visit);
  };
  visit(node.block);
  return fails;
}

function suppressedSessionReadsInSource(text: string, label: string): readonly string[] {
  const source = ts.createSourceFile(label, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  const findings: string[] = [];
  const inspect = (text: string, label: string): void => {
    const embedded = ts.createSourceFile(label, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
    const visitEmbedded = (node: ts.Node): void => {
      if (ts.isCatchClause(node) && ts.isTryStatement(node.parent)) {
        const guarded = node.parent.tryBlock.getText(embedded);
        if (
          /(?:\.sessions\.(?:open|get|list)|\.(?:events|files|messages)\.list)\s*\(/.test(guarded) &&
          !catchFailsChildProcess(node, embedded)
        ) {
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
      inspect(templateSource(node), `${label}:${line}`);
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  return findings;
}

function suppressedSessionReads(path: string): readonly string[] {
  return suppressedSessionReadsInSource(readRepoFile(path), path);
}

describe("live scenario reliability", () => {
  it("resolves every live user-test entrypoint with whole-scenario retries disabled", () => {
    const packageJson = JSON.parse(readRepoFile("apps/user-tests/package.json")) as {
      scripts?: Record<string, string>;
    };
    const configs = new Set(
      Object.entries(packageJson.scripts ?? {})
        .filter(([name]) => name.startsWith("test:user") && name !== "test:user:offline")
        .map(([, command]) => /--config\s+(vitest(?:\.[\w-]+)?\.config\.ts)/.exec(command)?.[1])
        .filter((name): name is string => name !== undefined)
    );

    expect(configs.size).toBeGreaterThan(0);
    const names = [...configs];
    const configUrls = names.map((name) => pathToFileURL(resolve(repoRoot, "apps/user-tests", name)).href);
    const output = execFileSync(
      "bun",
      [
        "-e",
        "const urls=JSON.parse(process.env.AEX_LIVE_CONFIG_URLS);const values=[];for(const url of urls){const m=await import(url);values.push(m.default.test?.retry??0)};console.log(JSON.stringify(values))"
      ],
      {
        encoding: "utf8",
        env: { ...process.env, AEX_LIVE_CONFIG_URLS: JSON.stringify(configUrls) }
      }
    );
    const retries = JSON.parse(output) as number[];
    for (const [index, retry] of retries.entries()) {
      expect(retry, names[index]).toBe(0);
    }
  });

  it("does not neutralize live-test predicates with an always-true fallback", () => {
    for (const path of liveScenarioFiles()) {
      const source = readRepoFile(path);
      expect(source, path).not.toMatch(/\.filter\([^;\n]*\|\|\s*true\b/);
    }
  });

  it("does not suppress post-finish session reads", () => {
    for (const path of liveScenarioFiles()) {
      expect(suppressedSessionReads(path), path).toEqual([]);
    }
  });

  it("distinguishes swallowed reads from secret-safe child-process failure propagation", () => {
    const swallowed = `
      const script = \`try {
        await session.events.list();
      } catch (error) {
        process.stdout.write(JSON.stringify({ error: String(error) }));
      }\`;
    `;
    const propagated = `
      const script = \`try {
        await session.events.list();
      } catch (error) {
        process.stdout.write(JSON.stringify({ hasError: true }));
        process.exitCode = 1;
      }\`;
    `;

    expect(suppressedSessionReadsInSource(swallowed, "swallowed.ts")).toEqual(["swallowed.ts:2"]);
    expect(suppressedSessionReadsInSource(propagated, "propagated.ts")).toEqual([]);
  });

  it("keeps file-level dynamic fanout at one worker per job", () => {
    const workflow = readWorkflow(".github/workflows/live-user-tests.yml");
    const job = workflowJob(workflow, "live-user-tests");
    const step = workflowStepRunning(job, /\btest:user:files\b/);

    expect(jobNeeds(job)).toEqual(
      expect.arrayContaining(["prepare-artifact", "live-user-tests-preflight", "prepare-live-test-matrix"])
    );
    expect(job.strategy?.matrix?.include).toBe("${{ fromJSON(needs.prepare-live-test-matrix.outputs.test_matrix) }}");
    expect(job.strategy?.["max-parallel"]).toBeUndefined();
    expect(step.env?.AEX_USER_TEST_MAX_WORKERS).toBe(1);
    expect(step.env?.TEST_FILE).toBe("${{ matrix.file }}");
    expect(step.run).toMatch(/\btest:user:files\b/);
    expect(step.run).toMatch(/\$TEST_FILE/);
    expect(step.run).not.toMatch(/--shard(?:\s|$)/);
  });
});
