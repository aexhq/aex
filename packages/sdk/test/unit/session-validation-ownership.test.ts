import { readFileSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";
import { describe, expect, it } from "vitest";

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

describe("session validation diagnostic ownership", () => {
  it("has one catch-and-translate owner and no swallowed or raw parser causes", () => {
    const translatedCatches: string[] = [];
    const rawValidationCauseSites: string[] = [];
    for (const path of productionSources()) {
      const text = readFileSync(path, "utf8");
      const source = ts.createSourceFile(path, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
      for (const node of descendants(source)) {
        if (!ts.isCatchClause(node)) continue;
        const identifiers = descendants(node.block)
          .filter(ts.isIdentifier)
          .map((identifier) => identifier.text);
        if (identifiers.includes("configError")) translatedCatches.push(path);
      }
      if (
        (path.endsWith("client.ts") || path.endsWith("session-validate.ts")) &&
        /cause\s*:\s*(?:err|error|caught)\b/.test(text)
      ) {
        rawValidationCauseSites.push(path);
      }
    }

    expect(translatedCatches).toEqual([join(sourceRoot, "session-validate.ts")]);
    expect(rawValidationCauseSites).toEqual([]);
    expect(readFileSync(join(sourceRoot, "client.ts"), "utf8")).not.toContain("void err;");
  });

  it("keeps one private adapter/sanitizer owner outside supported exports", async () => {
    const validation = readFileSync(join(sourceRoot, "session-validate.ts"), "utf8");
    const parsed = ts.createSourceFile(
      "session-validate.ts",
      validation,
      ts.ScriptTarget.Latest,
      true,
      ts.ScriptKind.TS
    );
    const functions = descendants(parsed)
      .filter(ts.isFunctionDeclaration)
      .map((node) => node.name?.text);
    expect(functions.filter((name) => name === "validatedSessionConfig")).toHaveLength(1);
    expect(functions.filter((name) => name === "safeValidationDiagnostic")).toHaveLength(1);

    const client = readFileSync(join(sourceRoot, "client.ts"), "utf8");
    expect(client).toMatch(/import\s*\{[\s\S]*validatedSessionConfig[\s\S]*\}\s*from\s*"\.\/session-validate\.js"/);
    const root = await import("../../src/index.js");
    expect(root).not.toHaveProperty("validatedSessionConfig");
    expect(root).not.toHaveProperty("safeValidationDiagnostic");
    expect(root).not.toHaveProperty("SessionConfigDiagnosticError");
  }, 15_000);
});

describe("session option-key ownership", () => {
  const exactTupleNames = [
    "SESSION_CREATE_KEYS",
    "SESSION_START_KEYS",
    "START_CONTROL_KEYS",
    "SESSION_SEND_KEYS",
    "SESSION_STREAM_KEYS",
    "SESSION_OVERRIDE_KEYS",
    "SESSION_RUNTIME_KEYS",
    "FILE_CAPTURE_KEYS",
    "SESSION_WEBHOOK_KEYS",
    "SESSION_ENVIRONMENT_KEYS",
    "PLATFORM_NETWORKING_KEYS",
    "PLATFORM_PACKAGE_INPUT_KEYS",
    "TEXT_RESPONSE_FORMAT_KEYS",
    "JSON_SCHEMA_RESPONSE_FORMAT_KEYS",
    "APPROVAL_GATE_KEYS",
    "ASSET_CATEGORY_KEYS"
  ] as const;

  it("keeps every closed runtime vocabulary beside one bidirectional type proof", () => {
    const path = join(sourceRoot, "session-validate.ts");
    const text = readFileSync(path, "utf8");
    const source = ts.createSourceFile(path, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
    const declarations = descendants(source).filter(ts.isVariableDeclaration);
    const declaredNames = new Set(
      declarations
        .map((node) => ts.isIdentifier(node.name) ? node.name.text : undefined)
        .filter((name): name is string => name !== undefined)
    );
    for (const name of [...exactTupleNames, "ASSET_ITEM_KEYS"]) {
      expect(declaredNames, `${name} must have one runtime owner`).toContain(name);
    }

    expect(text.match(/export type ExactKeySet</g)).toHaveLength(1);
    expect(text).toContain("Exclude<keyof Shape, Keys>");
    expect(text).toContain("Exclude<Keys, keyof Shape>");
    expect(text.match(/KeysAreExact\s*=\s*Assert<ExactKeySet</g)).toHaveLength(20);

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
    const source = ts.createSourceFile(
      "session-validate.ts",
      validation,
      ts.ScriptTarget.Latest,
      true,
      ts.ScriptKind.TS
    );
    const closedKeyLiterals = descendants(source)
      .filter(ts.isVariableDeclaration)
      .filter((node) => ts.isIdentifier(node.name) && (
        exactTupleNames.includes(node.name.text as typeof exactTupleNames[number]) ||
        node.name.text === "ASSET_ITEM_KEYS"
      ))
      .flatMap((node) => descendants(node.initializer ?? node).filter(ts.isStringLiteral).map((literal) => literal.text));
    for (const excluded of [
      "runtimeSize", "runtimeKind", "secretEnv", "parentSessionId", "prompt",
      "idleSuspendAfter", "signal", "from", "envVars", "ecosystem"
    ]) {
      expect(closedKeyLiterals, `${excluded} is guidance/internal vocabulary, never an allowed key`).not.toContain(excluded);
    }
    for (const openDictionary of ["METADATA_KEYS", "API_KEY_KEYS", "VARIABLE_KEYS", "SECRET_KEYS", "SCHEMA_KEYS"]) {
      expect(validation).not.toContain(openDictionary);
    }

    const client = readFileSync(join(sourceRoot, "client.ts"), "utf8");
    expect(client).not.toContain("assertAllowedObjectFields");
    expect(client).not.toMatch(/new Set\s*\(\s*\[/);
    const typeLeaf = readFileSync(join(sourceRoot, "client-types.ts"), "utf8");
    const parsedTypeLeaf = ts.createSourceFile("client-types.ts", typeLeaf, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
    expect(descendants(parsedTypeLeaf).filter(ts.isVariableStatement)).toEqual([]);

    const root = await import("../../src/index.js");
    expect(root).not.toHaveProperty("assertStartSessionOptions");
    expect(root).not.toHaveProperty("ExactKeySet");
  }, 15_000);
});
