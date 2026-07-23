import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";
import ts from "typescript";
import { CANONICAL_SHA256_DIGEST_PATTERN } from "../src/canonical-sha256.js";

const sourceDir = fileURLToPath(new URL("../src/", import.meta.url));

function sourceFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return sourceFiles(path);
    return entry.isFile() && entry.name.endsWith(".ts") ? [path] : [];
  });
}

function source(path: string): string {
  return readFileSync(join(sourceDir, path), "utf8");
}

function sourceFile(path: string): ts.SourceFile {
  return ts.createSourceFile(path, source(path), ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
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

function importedCanonicalSymbol(file: ts.SourceFile): boolean {
  return file.statements.some((statement) => {
    if (ts.isImportDeclaration(statement)) {
      if (!ts.isStringLiteral(statement.moduleSpecifier) || statement.moduleSpecifier.text !== "./canonical-sha256.js") return false;
      const bindings = statement.importClause?.namedBindings;
      return bindings !== undefined && ts.isNamedImports(bindings) && bindings.elements.some((element) =>
        (element.propertyName?.text ?? element.name.text) === "CANONICAL_SHA256_DIGEST_PATTERN"
      );
    }
    if (ts.isExportDeclaration(statement)) {
      if (!statement.moduleSpecifier || !ts.isStringLiteral(statement.moduleSpecifier) || statement.moduleSpecifier.text !== "./canonical-sha256.js") return false;
      const clause = statement.exportClause;
      return clause !== undefined && ts.isNamedExports(clause) && clause.elements.some((element) =>
        (element.propertyName?.text ?? element.name.text) === "CANONICAL_SHA256_DIGEST_PATTERN"
      );
    }
    return false;
  });
}

function declaredCanonicalSymbol(file: ts.SourceFile): ts.VariableDeclaration | undefined {
  return descendants(file)
    .filter(ts.isVariableDeclaration)
    .find((declaration) => ts.isIdentifier(declaration.name) && declaration.name.text === "CANONICAL_SHA256_DIGEST_PATTERN");
}

function namedVariable(file: ts.SourceFile, name: string): ts.VariableDeclaration | undefined {
  return descendants(file)
    .filter(ts.isVariableDeclaration)
    .find((declaration) => ts.isIdentifier(declaration.name) && declaration.name.text === name);
}

function functionByName(file: ts.SourceFile, name: string): ts.FunctionDeclaration | undefined {
  return file.statements
    .filter(ts.isFunctionDeclaration)
    .find((declaration) => declaration.name?.text === name);
}

describe("canonical SHA-256 digest ownership", () => {
  it("has exactly one public-contracts source owner for the prefixed grammar", () => {
    const ownerFiles = sourceFiles(sourceDir)
      .filter((file) => declaredCanonicalSymbol(sourceFile(relative(sourceDir, file).replaceAll("\\", "/"))) !== undefined);
    expect(ownerFiles).toHaveLength(1);
    expect(relative(sourceDir, ownerFiles[0]!).replaceAll("\\", "/")).toBe("canonical-sha256.ts");
    const ownerSource = sourceFile("canonical-sha256.ts");
    const owner = declaredCanonicalSymbol(ownerSource);
    expect(ownerSource.statements.filter(ts.isImportDeclaration)).toEqual([]);
    expect(ownerSource.statements.some((statement) =>
      ts.isVariableStatement(statement) &&
      statement.modifiers?.some((modifier) => modifier.kind === ts.SyntaxKind.ExportKeyword) &&
      statement.declarationList.declarations.includes(owner!)
    )).toBe(true);
    expect(CANONICAL_SHA256_DIGEST_PATTERN.test("sha256:" + "a".repeat(64))).toBe(true);
    expect(CANONICAL_SHA256_DIGEST_PATTERN.test("sha256:" + "A".repeat(64))).toBe(false);
    expect(CANONICAL_SHA256_DIGEST_PATTERN.test("sha256:" + "a".repeat(63))).toBe(false);
  });

  it("routes canonical digest consumers through the neutral owner", () => {
    const consumers = sourceFiles(sourceDir)
      .map((file) => ({ file, relativePath: relative(sourceDir, file).replaceAll("\\", "/") }))
      .filter(({ relativePath }) => relativePath !== "canonical-sha256.ts")
      .filter(({ file }) => importedCanonicalSymbol(sourceFile(relative(sourceDir, file).replaceAll("\\", "/"))))
      .map(({ relativePath }) => relativePath);
    expect(consumers).toEqual(expect.arrayContaining(["operations.ts", "session-config.ts", "workspace-resources.ts"]));
    expect(consumers).not.toContain("canonical-sha256.ts");
  });

  it("leaves the distinct bare session-file checksum grammar local", () => {
    const operations = sourceFile("operations.ts");
    const localPattern = namedVariable(operations, "SESSION_FILE_SHA256_PATTERN");
    expect(localPattern).toBeDefined();
    const localStatement = operations.statements.find((statement) =>
      ts.isVariableStatement(statement) && statement.declarationList.declarations.includes(localPattern!)
    );
    expect(localStatement && ts.isVariableStatement(localStatement) ? localStatement.modifiers ?? [] : []).toEqual([]);
    expect(localPattern?.initializer && ts.isRegularExpressionLiteral(localPattern.initializer)).toBe(true);
    const sessionFileListing = functionByName(operations, "listSessionFiles");
    expect(sessionFileListing).toBeDefined();
    expect(descendants(sessionFileListing!).some((node) => ts.isIdentifier(node) && node.text === "SESSION_FILE_SHA256_PATTERN")).toBe(true);
  });
});
