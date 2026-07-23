import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";

const here = dirname(fileURLToPath(import.meta.url));
const sourceRoot = resolve(here, "..", "..", "src");
const privateLeaves = ["client-types", "event-projection", "session-validate", "submission-wire"] as const;
const modules = [...privateLeaves, "client"] as const;

function sourceFile(name: string): ts.SourceFile {
  return ts.createSourceFile(
    `${name}.ts`,
    readFileSync(resolve(sourceRoot, `${name}.ts`), "utf8"),
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TS
  );
}

function relativeImports(name: string): readonly string[] {
  const file = sourceFile(name);
  const imports: string[] = [];
  for (const statement of file.statements) {
    if (!ts.isImportDeclaration(statement) || !ts.isStringLiteral(statement.moduleSpecifier)) continue;
    const specifier = statement.moduleSpecifier.text;
    if (specifier.startsWith("./")) imports.push(specifier.slice(2).replace(/\.js$/, ""));
  }
  return imports;
}

function isExported(node: ts.Node): boolean {
  return !!node.modifiers?.some((modifier) => modifier.kind === ts.SyntaxKind.ExportKeyword);
}

function descendants(node: ts.Node): readonly ts.Node[] {
  const out: ts.Node[] = [];
  const visit = (current: ts.Node): void => {
    out.push(current);
    current.forEachChild(visit);
  };
  visit(node);
  return out;
}

function exportedFunctionNames(file: ts.SourceFile): readonly string[] {
  return file.statements
    .filter((statement): statement is ts.FunctionDeclaration => ts.isFunctionDeclaration(statement))
    .filter(isExported)
    .map((statement) => statement.name?.text)
    .filter((name): name is string => name !== undefined);
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
    const file = sourceFile("client");
    const classes = file.statements
      .filter(ts.isClassDeclaration)
      .filter(isExported)
      .map((node) => node.name?.text);
    expect(classes.filter((name): name is string => name !== undefined)).toContain("Aex");
    expect(classes.length).toBeGreaterThan(1);

    const clientFunctions = new Set(
      file.statements
        .filter(ts.isFunctionDeclaration)
        .map((node) => node.name?.text)
        .filter((name): name is string => name !== undefined)
    );
    for (const leaf of privateLeaves) {
      const leafFile = sourceFile(leaf);
      expect(leafFile.statements.filter((statement) => ts.isClassDeclaration(statement) && isExported(statement))).toEqual([]);
      for (const name of exportedFunctionNames(leafFile)) {
        expect(clientFunctions, `${name} must remain owned by ${leaf}.ts`).not.toContain(name);
      }
    }
  });

  it("keeps extracted helpers reachable through their private leaves", () => {
    const client = sourceFile("client");
    const importedModules = new Set(
      client.statements
        .filter(ts.isImportDeclaration)
        .map((statement) => ts.isStringLiteral(statement.moduleSpecifier) ? statement.moduleSpecifier.text : undefined)
        .filter((specifier): specifier is string => specifier !== undefined)
    );
    for (const leaf of privateLeaves.filter((name) => name !== "client-types")) {
      expect(importedModules, `client.ts must depend on ${leaf}.ts`).toContain(`./${leaf}.js`);
    }
  });
});
