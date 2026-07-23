import { readFileSync } from "node:fs";
import { describe, expect, it } from "bun:test";
import ts from "typescript";

const sourceText = readFileSync(new URL("../src/asset-bundle.ts", import.meta.url), "utf8");
const source = ts.createSourceFile("asset-bundle.ts", sourceText, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);

function descendants(node: ts.Node): readonly ts.Node[] {
  const out: ts.Node[] = [];
  const visit = (current: ts.Node): void => {
    out.push(current);
    current.forEachChild(visit);
  };
  visit(node);
  return out;
}

function functionDeclaration(name: string): ts.FunctionDeclaration {
  const declaration = source.statements.find((statement) =>
    ts.isFunctionDeclaration(statement) && statement.name?.text === name
  );
  if (!declaration || !ts.isFunctionDeclaration(declaration)) throw new Error(`missing ${name}`);
  return declaration;
}

function callNames(node: ts.Node): readonly string[] {
  return descendants(node)
    .filter(ts.isCallExpression)
    .map((call) => ts.isIdentifier(call.expression) ? call.expression.text : undefined)
    .filter((name): name is string => name !== undefined);
}

function memberCallNames(node: ts.Node): readonly string[] {
  return descendants(node)
    .filter(ts.isCallExpression)
    .map((call) => ts.isPropertyAccessExpression(call.expression) ? call.expression.name.text : undefined)
    .filter((name): name is string => name !== undefined);
}

function propertyNames(node: ts.Node): readonly string[] {
  return descendants(node)
    .filter(ts.isPropertyAssignment)
    .map((property) => ts.isIdentifier(property.name) || ts.isStringLiteral(property.name) ? property.name.text : undefined)
    .filter((name): name is string => name !== undefined);
}

function hasExportModifier(node: ts.FunctionDeclaration): boolean {
  return !!node.modifiers?.some((modifier) => modifier.kind === ts.SyntaxKind.ExportKeyword);
}

describe("canonical asset bundle pipeline ownership", () => {
  it("keeps entry collection and archive finalization in one shared pipeline", () => {
    const canonical = functionDeclaration("bundleCanonicalFiles");
    expect(hasExportModifier(canonical)).toBe(false);
    const calls = callNames(canonical);
    for (const owner of ["parseSkillBundleEntry", "validateBundleGraph", "buildCanonicalZippable", "zipSync"]) {
      expect(calls, `${owner} must stay in the canonical pipeline`).toContain(owner);
    }
    expect(descendants(canonical).some((node) => ts.isForOfStatement(node) && ts.isIdentifier(node.expression) && node.expression.text === "entries")).toBe(true);
    expect(memberCallNames(canonical)).toContain("encode");
    expect(descendants(canonical).some((node) => ts.isIdentifier(node) && node.text === "totalDecompressed")).toBe(true);
    expect(memberCallNames(canonical)).toContain("has");
    expect(propertyNames(canonical)).toEqual(expect.arrayContaining(["zip", "fileCount", "compressedSize"]));
  });

  it("keeps the skill and tool exports as policy-only adapters", () => {
    for (const name of ["bundleSkillFiles", "bundleToolFiles"]) {
      const adapter = functionDeclaration(name);
      expect(hasExportModifier(adapter)).toBe(true);
      expect(callNames(adapter)).toContain("bundleCanonicalFiles");
      expect(descendants(adapter).some(ts.isForOfStatement)).toBe(false);
      expect(callNames(adapter)).not.toContain("zipSync");
    }
  });
});
