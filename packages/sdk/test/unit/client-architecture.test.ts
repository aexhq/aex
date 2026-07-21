import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";

const here = dirname(fileURLToPath(import.meta.url));
const sourceRoot = resolve(here, "..", "..", "src");
const privateLeaves = ["client-types", "event-projection", "session-validate", "submission-wire"] as const;
const modules = [...privateLeaves, "client"] as const;

function source(name: string): string {
  return readFileSync(resolve(sourceRoot, `${name}.ts`), "utf8");
}

function relativeImports(name: string): readonly string[] {
  const file = ts.createSourceFile(`${name}.ts`, source(name), ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  const imports: string[] = [];
  for (const statement of file.statements) {
    if (!ts.isImportDeclaration(statement) || !ts.isStringLiteral(statement.moduleSpecifier)) continue;
    const specifier = statement.moduleSpecifier.text;
    if (specifier.startsWith("./")) imports.push(specifier.slice(2).replace(/\.js$/, ""));
  }
  return imports;
}

describe("SDK client module architecture", () => {
  it("keeps private leaves acyclic and independent of client/index orchestration", () => {
    const graph = new Map(modules.map((name) => [name, relativeImports(name).filter((dep) => modules.includes(dep as never))]));
    for (const leaf of privateLeaves) {
      expect(relativeImports(leaf), `${leaf} must not import an orchestration root`).not.toContain("client");
      expect(relativeImports(leaf), `${leaf} must not import the package barrel`).not.toContain("index");
    }

    const visiting = new Set<string>();
    const visited = new Set<string>();
    const visit = (name: string): void => {
      if (visiting.has(name)) throw new Error(`SDK client module cycle reaches ${name}`);
      if (visited.has(name)) return;
      visiting.add(name);
      for (const dependency of graph.get(name as typeof modules[number]) ?? []) visit(dependency);
      visiting.delete(name);
      visited.add(name);
    };
    for (const name of modules) visit(name);
  });

  it("keeps all exported client classes in client.ts and extracted helpers out", () => {
    const file = ts.createSourceFile("client.ts", source("client"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
    const classes = file.statements
      .filter(ts.isClassDeclaration)
      .filter((node) => node.modifiers?.some((modifier) => modifier.kind === ts.SyntaxKind.ExportKeyword))
      .map((node) => node.name?.text);
    expect(classes).toEqual([
      "SessionRunStream", "SessionHandle", "SessionClient", "ChildSessionHandle", "SecretsClient",
      "OrgsClient", "WorkspacesClient", "KeysClient", "Aex", "WorkspaceFilesClient",
      "WorkspaceSkillsClient", "WorkspaceToolsClient", "WorkspaceInstructionsClient", "WorkspaceClient"
    ]);

    const functions = new Set(file.statements.filter(ts.isFunctionDeclaration).map((node) => node.name?.text));
    for (const moved of [
      "messageFromWire", "projectAssistantMessages", "turnTraceFromEvents", "buildTurnResult",
      "normaliseSessionInput", "assertSupportedSessionFields", "assertSupportedSessionSendOptions",
      "fileCaptureForWire", "sessionRetentionForWire", "sessionEnvironmentForWire", "mergeMcpServers"
    ]) {
      expect(functions.has(moved), `${moved} must remain owned by a private leaf`).toBe(false);
    }
  });

  it("enforces focused source budgets for the decomposed modules", () => {
    const budgets: Readonly<Record<typeof modules[number], number>> = {
      client: 1_900,
      "client-types": 210,
      "event-projection": 475,
      "session-validate": 390,
      "submission-wire": 125
    };
    for (const [name, budget] of Object.entries(budgets)) {
      const lines = source(name).split(/\r?\n/).length;
      expect(lines, `${name}.ts exceeds its focused architecture budget of ${budget} lines`).toBeLessThanOrEqual(budget);
    }
  });
});
