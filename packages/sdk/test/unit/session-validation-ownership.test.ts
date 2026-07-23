import { readFileSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";
import { describe, expect, it } from "bun:test";

const sourceRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "src");

function productionSources(): readonly string[] {
  return readdirSync(sourceRoot)
    .filter((name) => name.endsWith(".ts"))
    .map((name) => join(sourceRoot, name));
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

function sourceFile(path: string, text: string): ts.SourceFile {
  return ts.createSourceFile(path, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
}

function identifierName(node: ts.Node | undefined): string | undefined {
  if (node === undefined) return undefined;
  if (ts.isIdentifier(node)) return node.text;
  if (ts.isPropertyAccessExpression(node)) return identifierName(node.name);
  return undefined;
}

function rootIdentifierName(node: ts.Node | undefined): string | undefined {
  if (node === undefined) return undefined;
  if (ts.isIdentifier(node)) return node.text;
  if (ts.isPropertyAccessExpression(node)) return rootIdentifierName(node.expression);
  if (ts.isQualifiedName(node)) return rootIdentifierName(node.left);
  return undefined;
}

function hasNamedImport(source: ts.SourceFile, moduleName: string, importedName: string): boolean {
  return source.statements
    .filter(ts.isImportDeclaration)
    .filter((statement) => ts.isStringLiteral(statement.moduleSpecifier) && statement.moduleSpecifier.text === moduleName)
    .some((statement) => {
      const bindings = statement.importClause?.namedBindings;
      if (!bindings || !ts.isNamedImports(bindings)) return false;
      return bindings.elements.some((element) => (element.propertyName?.text ?? element.name.text) === importedName);
    });
}

function hasRawCauseObject(node: ts.Node): boolean {
  return descendants(node)
    .filter(ts.isPropertyAssignment)
    .some((property) => identifierName(property.name) === "cause" && ts.isIdentifier(property.initializer) &&
      ["err", "error", "caught"].includes(property.initializer.text));
}

function hasVoidIdentifier(node: ts.Node): boolean {
  // `void err` parses as a VoidExpression (not a PrefixUnaryExpression, whose
  // operator union excludes `void`) — the old filter made this guard vacuous.
  return descendants(node)
    .filter(ts.isVoidExpression)
    .some((expression) => ts.isIdentifier(expression.expression));
}

function isExactKeyOwner(name: string): boolean {
  return name.endsWith("_KEYS") || name === "ASSET_ITEM_KEYS";
}

function hasExactKeyProof(node: ts.TypeAliasDeclaration, owner: string): boolean {
  const hasAssert = descendants(node.type)
    .filter(ts.isTypeReferenceNode)
    .some((reference) => identifierName(reference.typeName) === "Assert");
  const hasOwner = descendants(node.type)
    .filter(ts.isTypeQueryNode)
    .some((query) => rootIdentifierName(query.exprName) === owner);
  return hasAssert && hasOwner;
}

describe("session validation diagnostic ownership", () => {
  it("has one catch-and-translate owner and no swallowed or raw parser causes", () => {
    const translatedCatches: string[] = [];
    const rawValidationCauseSites: string[] = [];
    for (const path of productionSources()) {
      const text = readFileSync(path, "utf8");
      const source = sourceFile(path, text);
      for (const node of descendants(source)) {
        if (!ts.isCatchClause(node)) continue;
        const identifiers = descendants(node.block)
          .filter(ts.isIdentifier)
          .map((identifier) => identifier.text);
        if (identifiers.includes("configError") && descendants(node.block).some(ts.isCallExpression)) {
          translatedCatches.push(path);
        }
        if (hasVoidIdentifier(node.block)) rawValidationCauseSites.push(path);
      }
      if (
        (path.endsWith("client.ts") || path.endsWith("session-validate.ts")) &&
        hasRawCauseObject(source)
      ) {
        rawValidationCauseSites.push(path);
      }
    }

    expect(translatedCatches).toHaveLength(1);
    expect(translatedCatches[0]).toBe(join(sourceRoot, "session-validate.ts"));
    expect(rawValidationCauseSites).toEqual([]);
  });

  it("keeps one private adapter/sanitizer owner outside supported exports", async () => {
    const validation = readFileSync(join(sourceRoot, "session-validate.ts"), "utf8");
    const parsed = sourceFile("session-validate.ts", validation);
    const functions = descendants(parsed)
      .filter(ts.isFunctionDeclaration)
      .map((node) => node.name?.text);
    expect(functions.filter((name) => name === "validatedSessionConfig")).toHaveLength(1);
    expect(functions.filter((name) => name === "safeValidationDiagnostic")).toHaveLength(1);

    const client = readFileSync(join(sourceRoot, "client.ts"), "utf8");
    expect(hasNamedImport(sourceFile("client.ts", client), "./session-validate.js", "validatedSessionConfig")).toBe(true);
    const root = await import("../../src/index.js");
    expect(root).not.toHaveProperty("validatedSessionConfig");
    expect(root).not.toHaveProperty("safeValidationDiagnostic");
    expect(root).not.toHaveProperty("SessionConfigDiagnosticError");
  }, 15_000);
});

describe("session option-key ownership", () => {

  it("keeps every closed runtime vocabulary beside one bidirectional type proof", () => {
    const path = join(sourceRoot, "session-validate.ts");
    const text = readFileSync(path, "utf8");
    const source = sourceFile(path, text);
    const declarations = descendants(source).filter(ts.isVariableDeclaration);
    const keyOwners = declarations
      .filter((node) => ts.isIdentifier(node.name) && isExactKeyOwner(node.name.text))
      .filter((node): node is ts.VariableDeclaration & { name: ts.Identifier } => ts.isIdentifier(node.name));
    expect(keyOwners.length).toBeGreaterThan(0);
    for (const owner of keyOwners) {
      expect(owner.initializer, `${owner.name.text} must have a runtime initializer`).toBeDefined();
      expect(
        descendants(source).filter(ts.isTypeAliasDeclaration).some((alias) => hasExactKeyProof(alias, owner.name.text)),
        `${owner.name.text} must have an exact type proof`
      ).toBe(true);
    }

    const exactKeySet = source.statements
      .filter(ts.isTypeAliasDeclaration)
      .filter((alias) => alias.name.text === "ExactKeySet");
    expect(exactKeySet).toHaveLength(1);

    const inlineAllowedKeyCalls = descendants(source)
      .filter(ts.isCallExpression)
      .filter((node) => {
        const called = ts.isIdentifier(node.expression) ? node.expression.text : "";
        return called === "assertAllowedKeys" || called === "assertAllowedObjectFields";
      })
      .filter((node) => node.arguments.some(ts.isArrayLiteralExpression));
    expect(inlineAllowedKeyCalls).toEqual([]);

    const literalSets = descendants(source)
      .filter(ts.isNewExpression)
      .filter((node) => ts.isIdentifier(node.expression) && node.expression.text === "Set")
      .filter((node) => node.arguments?.some(ts.isArrayLiteralExpression));
    expect(literalSets).toEqual([]);
  });

  it("keeps aliases, wire-only fields, and open dictionaries outside allowed tuples", async () => {
    const validation = readFileSync(join(sourceRoot, "session-validate.ts"), "utf8");
    const source = sourceFile("session-validate.ts", validation);
    const closedKeyLiterals = descendants(source)
      .filter(ts.isVariableDeclaration)
      .filter((node) => ts.isIdentifier(node.name) && isExactKeyOwner(node.name.text))
      .flatMap((node) => descendants(node.initializer ?? node).filter(ts.isStringLiteral).map((literal) => literal.text));
    for (const excluded of [
      "runtimeSize", "runtimeKind", "secretEnv", "parentSessionId", "prompt",
      "idleSuspendAfter", "signal", "from", "envVars", "ecosystem"
    ]) {
      expect(closedKeyLiterals, `${excluded} is guidance/internal vocabulary, never an allowed key`).not.toContain(excluded);
    }
    const declarationsByName = new Set(
      descendants(source)
        .filter(ts.isVariableDeclaration)
        .map((node) => ts.isIdentifier(node.name) ? node.name.text : undefined)
        .filter((name): name is string => name !== undefined)
    );
    for (const openDictionary of ["METADATA_KEYS", "API_KEY_KEYS", "VARIABLE_KEYS", "SECRET_KEYS", "SCHEMA_KEYS"]) {
      expect(declarationsByName).not.toContain(openDictionary);
    }

    const client = readFileSync(join(sourceRoot, "client.ts"), "utf8");
    const clientSource = sourceFile("client.ts", client);
    expect(descendants(clientSource).filter(ts.isCallExpression).some((call) =>
      ts.isIdentifier(call.expression) && call.expression.text === "assertAllowedObjectFields"
    )).toBe(false);
    expect(descendants(clientSource).filter(ts.isNewExpression).some((expression) =>
      ts.isIdentifier(expression.expression) && expression.expression.text === "Set" &&
      expression.arguments?.some(ts.isArrayLiteralExpression)
    )).toBe(false);
    const typeLeaf = readFileSync(join(sourceRoot, "client-types.ts"), "utf8");
    const parsedTypeLeaf = sourceFile("client-types.ts", typeLeaf);
    expect(descendants(parsedTypeLeaf).filter(ts.isVariableStatement)).toEqual([]);

    const root = await import("../../src/index.js");
    expect(root).not.toHaveProperty("assertStartSessionOptions");
    expect(root).not.toHaveProperty("ExactKeySet");
  }, 15_000);
});
