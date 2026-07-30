import { readdirSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";
import { describe, expect, it } from "bun:test";
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

function catchRecordsDiagnostic(node: ts.CatchClause, source: ts.SourceFile): boolean {
  // A post-finish read is still observable when a scenario records the failure
  // in a named diagnostic field (for example snapshot.eventsError). This is
  // different from swallowing it into a generic `error` string.
  const block = node.block.getText(source);
  const parameter = node.variableDeclaration?.name && ts.isIdentifier(node.variableDeclaration.name)
    ? node.variableDeclaration.name.text
    : undefined;
  return /\.[A-Za-z_$][\w$]*(?:Error|Err|Thrown)\s*=/.test(block) ||
    /\b(?:error|[A-Za-z_$][\w$]*Error)\s*=\s*/.test(block) ||
    /\b(?:err|error|failure|diagnostic)\s*=\s*[A-Za-z_$][\w$]*\s*\(/.test(block) ||
    (parameter !== undefined && new RegExp(`\\.(?:push|add|set)\\([^)]*\\b${parameter}\\b`).test(block));
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
          !catchFailsChildProcess(node, embedded) &&
          !catchRecordsDiagnostic(node, embedded)
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

// `bun test --retry=N` (or `--retry N`) re-runs a failing test whole-scenario —
// the exact behavior the retired vitest lane configs locked to `retry: 0`. With
// the lane configs gone, the retry surface is the lane SCRIPT argv plus the
// lane runner's own bun-test argv assembly, so the lock moves there.
const RETRY_FLAG = /(?:^|\s)--retry(?:[=\s]|$)/;

describe("live scenario reliability", () => {
  it("keeps whole-scenario retries disabled in every user-test lane", () => {
    const laneScripts = Object.entries(
      (JSON.parse(readRepoFile("apps/user-tests/package.json")) as {
        scripts?: Record<string, string>;
      }).scripts ?? {}
    ).filter(([name]) => name.startsWith("test:user"));

    expect(laneScripts.length).toBeGreaterThan(0);
    for (const [name, command] of laneScripts) {
      expect(command, name).not.toMatch(RETRY_FLAG);
    }
    // The lane runner forwards its argv to `bun test` verbatim and assembles
    // the rest itself; it must never introduce a retry flag on its own.
    expect(readRepoFile("apps/user-tests/scripts/user-bun-test.mjs")).not.toMatch(RETRY_FLAG);
    // The two direct `bun test` lanes (fuzz/e2e) are covered by the script scan
    // above; the shared bunfig must not smuggle retries in either.
    expect(readRepoFile("apps/user-tests/bunfig.toml")).not.toMatch(RETRY_FLAG);
  });

  it("detects both --retry spellings the lane scripts could smuggle in", () => {
    expect("bun test --isolate --retry=2 test/offline").toMatch(RETRY_FLAG);
    expect("bun test --isolate --retry 2 test/offline").toMatch(RETRY_FLAG);
    expect("bun test --isolate --retry").toMatch(RETRY_FLAG);
    // No false positives on flags that merely contain the word.
    expect("bun test --isolate --retry-free test/offline").not.toMatch(RETRY_FLAG);
    expect("bun scripts/user-bun-test.mjs --isolate --timeout=180000 --sweep").not.toMatch(RETRY_FLAG);
  });

  it("does not neutralize live-test predicates with an always-true fallback", () => {
    for (const path of liveScenarioFiles()) {
      const source = readRepoFile(path);
      expect(source, path).not.toMatch(/\.filter\([^;\n]*\|\|\s*true\b/);
    }
  });

  it("awaits the terminal run, performs explicit cleanup, and suppresses no session reads", () => {
    const files = liveScenarioFiles();
    expect(files).toEqual(["apps/user-tests/test/live/v1-session.user.test.ts"]);
    for (const path of files) {
      const source = readRepoFile(path);
      expect(source).toContain("accepted.run.result(");
      expect(source).toContain("session.delete({ cascade: true })");
      expect(source).toContain("deletion.result(");
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

    expect(jobNeeds(job)).toEqual(["prepare-artifacts", "preflight"]);
    expect(job.strategy?.matrix?.include).toBe("${{ fromJSON(needs.prepare-artifacts.outputs.matrix) }}");
    expect(job.strategy?.["max-parallel"]).toBeUndefined();
    expect(step.env?.TEST_FILES).toBe("${{ matrix.file }}");
    expect(step.run).toMatch(/\btest:user:files\b/);
    expect(step.run).toMatch(/\$TEST_FILES/);
    expect(step.run).not.toMatch(/--shard(?:\s|$)/);
  });
});
