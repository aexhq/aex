import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, extname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";
import { describe, expect, it } from "vitest";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const SOURCE_EXTENSIONS = new Set([".ts", ".tsx", ".cts", ".mts", ".js", ".jsx", ".cjs", ".mjs"]);

describe("fast-check execution policy", () => {
  it("gives every property assertion an explicit numRuns bound", () => {
    const offenders: string[] = [];

    for (const path of sourceFiles()) {
      const source = readFileSync(resolve(repoRoot, path), "utf8");
      if (!source.includes("fc.assert")) continue;

      const sourceFile = ts.createSourceFile(path, source, ts.ScriptTarget.Latest, true);
      const declarations = variableInitializers(sourceFile);
      visit(sourceFile, (node) => {
        if (!isFastCheckAssert(node)) return;
        const options = node.arguments[1];
        if (options && hasNumRuns(options, declarations, new Set())) return;
        const { line } = sourceFile.getLineAndCharacterOfPosition(node.getStart(sourceFile));
        offenders.push(`${path}:${line + 1}`);
      });
    }

    expect(offenders).toEqual([]);
  });
});

function sourceFiles(): readonly string[] {
  return execFileSync("git", ["ls-files", "--cached", "--others", "--exclude-standard", "-z"], {
    cwd: repoRoot,
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024
  })
    .split("\0")
    .filter((path) => path !== "" && SOURCE_EXTENSIONS.has(extname(path)) && existsSync(resolve(repoRoot, path)))
    .sort((left, right) => (left < right ? -1 : left > right ? 1 : 0));
}

function variableInitializers(sourceFile: ts.SourceFile): ReadonlyMap<string, ts.Expression> {
  const declarations = new Map<string, ts.Expression>();
  visit(sourceFile, (node) => {
    if (!ts.isVariableDeclaration(node) || !ts.isIdentifier(node.name) || !node.initializer) return;
    declarations.set(node.name.text, node.initializer);
  });
  return declarations;
}

function isFastCheckAssert(node: ts.Node): node is ts.CallExpression {
  return ts.isCallExpression(node)
    && ts.isPropertyAccessExpression(node.expression)
    && ts.isIdentifier(node.expression.expression)
    && node.expression.expression.text === "fc"
    && node.expression.name.text === "assert";
}

function hasNumRuns(
  expression: ts.Expression,
  declarations: ReadonlyMap<string, ts.Expression>,
  seen: Set<string>
): boolean {
  if (ts.isObjectLiteralExpression(expression)) {
    return expression.properties.some((property) => {
      if (!ts.isPropertyAssignment(property) && !ts.isShorthandPropertyAssignment(property)) return false;
      return property.name.getText() === "numRuns";
    });
  }
  if (ts.isIdentifier(expression)) {
    if (seen.has(expression.text)) return false;
    seen.add(expression.text);
    const initializer = declarations.get(expression.text);
    return initializer ? hasNumRuns(initializer, declarations, seen) : false;
  }
  if (ts.isAsExpression(expression) || ts.isSatisfiesExpression(expression) || ts.isParenthesizedExpression(expression)) {
    return hasNumRuns(expression.expression, declarations, seen);
  }
  return false;
}

function visit(node: ts.Node, callback: (node: ts.Node) => void): void {
  callback(node);
  ts.forEachChild(node, (child) => visit(child, callback));
}
