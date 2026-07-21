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
