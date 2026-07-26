import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import ts from "typescript";
import { describe, expect, it } from "bun:test";

const repoRoot = resolve(import.meta.dirname, "../..");
const generatedRoots = [
  resolve(repoRoot, "packages/contracts/dist"),
  resolve(repoRoot, "packages/sdk/dist/_contracts")
] as const;

function sourceFile(path: string): ts.SourceFile {
  return ts.createSourceFile(path, readFileSync(path, "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
}

function namedDeclaration(
  source: ts.SourceFile,
  name: string
): ts.Statement {
  const statement = source.statements.find((candidate) => {
    if (
      ts.isFunctionDeclaration(candidate) ||
      ts.isInterfaceDeclaration(candidate) ||
      ts.isTypeAliasDeclaration(candidate)
    ) {
      return candidate.name?.text === name;
    }
    if (ts.isVariableStatement(candidate)) {
      return candidate.declarationList.declarations.some(
        (declaration) => ts.isIdentifier(declaration.name) && declaration.name.text === name
      );
    }
    return false;
  });
  if (!statement) throw new Error(`missing declaration ${name} in ${source.fileName}`);
  return statement;
}

function variableDeclaration(statement: ts.Statement, name: string): ts.VariableDeclaration {
  if (!ts.isVariableStatement(statement)) throw new Error(`${name} is not a variable statement`);
  const declaration = statement.declarationList.declarations.find(
    (candidate) => ts.isIdentifier(candidate.name) && candidate.name.text === name
  );
  if (!declaration) throw new Error(`missing variable ${name}`);
  return declaration;
}

function jsDocComment(statement: ts.Statement, tagName: string): string | undefined {
  const tag = ts.getJSDocTags(statement).find((candidate) => candidate.tagName.text === tagName);
  if (!tag) return undefined;
  return typeof tag.comment === "string"
    ? tag.comment
    : tag.comment?.map((part) => part.text).join("");
}

describe("generated AssetRef public API", () => {
  it.each([...generatedRoots])("keeps one implementation and its compatibility declaration in %s", (root) => {
    const declarationPath = resolve(root, "session-config.d.ts");
    const declarationSource = sourceFile(declarationPath);

    expect(namedDeclaration(declarationSource, "AssetRef")).toSatisfy(ts.isInterfaceDeclaration);

    const fileRef = namedDeclaration(declarationSource, "FileRef");
    expect(ts.isTypeAliasDeclaration(fileRef)).toBe(true);
    if (!ts.isTypeAliasDeclaration(fileRef)) throw new Error("FileRef declaration changed kind");
    expect(ts.isTypeReferenceNode(fileRef.type) && fileRef.type.typeName.getText(declarationSource)).toBe("AssetRef");

    const canonical = namedDeclaration(declarationSource, "isAssetRef");
    expect(ts.isFunctionDeclaration(canonical)).toBe(true);
    if (!ts.isFunctionDeclaration(canonical)) throw new Error("isAssetRef declaration changed kind");
    expect(canonical.parameters).toHaveLength(1);
    expect(canonical.parameters[0]?.type?.getText(declarationSource)).toBe("FileRef");
    if (!canonical.type || !ts.isTypePredicateNode(canonical.type) || !canonical.type.type) {
      throw new Error("isAssetRef no longer emits a type predicate");
    }
    expect(canonical.type.type.getText(declarationSource)).toBe("AssetRef");
    expect(jsDocComment(canonical, "deprecated")).toBeUndefined();

    const compatibilityStatement = namedDeclaration(declarationSource, "isFileAssetRef");
    const compatibility = variableDeclaration(compatibilityStatement, "isFileAssetRef");
    expect(ts.isTypeQueryNode(compatibility.type!) && compatibility.type.exprName.getText(declarationSource)).toBe(
      "isAssetRef"
    );
    expect(jsDocComment(compatibilityStatement, "deprecated")).toContain("isAssetRef");

    const runtimeSource = sourceFile(resolve(root, "session-config.js"));
    const predicateImplementations = runtimeSource.statements.filter(
      (statement) => ts.isFunctionDeclaration(statement) &&
        (statement.name?.text === "isAssetRef" || statement.name?.text === "isFileAssetRef")
    );
    expect(predicateImplementations).toHaveLength(1);
    expect((predicateImplementations[0] as ts.FunctionDeclaration).name?.text).toBe("isAssetRef");

    const runtimeCompatibility = variableDeclaration(
      namedDeclaration(runtimeSource, "isFileAssetRef"),
      "isFileAssetRef"
    );
    expect(
      ts.isIdentifier(runtimeCompatibility.initializer!) && runtimeCompatibility.initializer.text
    ).toBe("isAssetRef");
  });

  it("keeps both generated root exports present and reference-identical", async () => {
    for (const root of generatedRoots) {
      const contracts = await import(pathToFileURL(resolve(root, "index.js")).href) as Record<string, unknown>;
      expect(contracts.isAssetRef).toBeTypeOf("function");
      expect(contracts.isFileAssetRef).toBe(contracts.isAssetRef);
    }
  });

  it("preserves the contracts package subpath closure", () => {
    const packageJson = JSON.parse(
      readFileSync(resolve(repoRoot, "packages/contracts/package.json"), "utf8")
    ) as { readonly exports?: Readonly<Record<string, unknown>> };
    // The same list `packages/contracts/test/public-entrypoints.test.ts` asserts,
    // which carries the reasoning per entry: `./ids` is the barrel-free id owner a
    // zero-`node_modules` bundle needs, and the two `./openapi/*` entries are
    // generated artefacts rather than code entrypoints.
    expect(Object.keys(packageJson.exports ?? {})).toEqual([
      ".",
      "./internal",
      "./ids",
      "./subagent-runtime",
      "./testing",
      "./openapi/data-plane.json",
      "./openapi/data-plane"
    ]);
  });
});
