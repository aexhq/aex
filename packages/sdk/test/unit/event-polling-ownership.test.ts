import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "bun:test";
import ts from "typescript";

const here = dirname(fileURLToPath(import.meta.url));
const sdkRoot = resolve(here, "..", "..");
const clientText = readFileSync(resolve(sdkRoot, "src", "client.ts"), "utf8");
const client = ts.createSourceFile("client.ts", clientText, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);

function topFunction(name: string): ts.FunctionDeclaration {
  const found = client.statements
    .filter(ts.isFunctionDeclaration)
    .find((candidate) => candidate.name?.text === name);
  if (!found) throw new Error(`missing ${name}`);
  return found;
}

function descendantsOfKind<T extends ts.Node>(node: ts.Node, guard: (candidate: ts.Node) => candidate is T): T[] {
  const found: T[] = [];
  const visit = (candidate: ts.Node): void => {
    if (guard(candidate)) found.push(candidate);
    ts.forEachChild(candidate, visit);
  };
  visit(node);
  return found;
}

describe("SDK event-polling ownership", () => {
  it("keeps list/filter/dedupe/yield/exit/sleep in one private async generator", () => {
    const engine = topFunction("pollSessionEventViews");
    const text = engine.getText(client);

    expect(engine.asteriskToken).toBeDefined();
    expect(engine.modifiers?.some((modifier) => modifier.kind === ts.SyntaxKind.AsyncKeyword)).toBe(true);
    expect(engine.modifiers?.some((modifier) => modifier.kind === ts.SyntaxKind.ExportKeyword)).not.toBe(true);
    expect(descendantsOfKind(engine, ts.isWhileStatement)).toHaveLength(1);
    expect(text.match(/operations\.listSessionEvents\(/g)).toHaveLength(1);
    expect(text).toContain("event.sequence >= from");
    expect(text).toContain("seenIds.add(event.id)");
    expect(text).toContain("yield asAexEventView(event)");
    expect(text).toContain("if (terminalSeen || boundedRunlessSnapshot) return");
    expect(text).toContain("await sleep(intervalMs, signal)");
  });

  it("keeps parent and child wrappers loop-free and delegated exactly once", () => {
    for (const name of ["streamSessionEventsPolling", "streamChildSessionEventsPolling"]) {
      const wrapper = topFunction(name);
      expect(descendantsOfKind(wrapper, ts.isWhileStatement), name).toHaveLength(0);
      expect(wrapper.getText(client).match(/pollSessionEventViews\(/g), name).toHaveLength(1);
      expect(wrapper.getText(client), name).not.toContain("operations.listSessionEvents");
      expect(wrapper.getText(client), name).not.toContain("seenIds");
      expect(wrapper.getText(client), name).not.toContain("sleep(");
    }
  });

  it("does not publish the polling engine or its resolved policy", () => {
    const indexText = readFileSync(resolve(sdkRoot, "src", "index.ts"), "utf8");
    const packageJson = JSON.parse(readFileSync(resolve(sdkRoot, "package.json"), "utf8")) as {
      readonly exports: Readonly<Record<string, unknown>>;
    };

    expect(indexText).not.toContain("pollSessionEventViews");
    expect(indexText).not.toContain("ResolvedSessionEventPolling");
    expect(Object.keys(packageJson.exports)).toEqual(["."]);
  });
});
