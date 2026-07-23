import { readFileSync, readdirSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "bun:test";
import { CLI_VERBS } from "../src/host/registry.js";

const here = dirname(fileURLToPath(import.meta.url));
const cliRoot = resolve(here, "..");
const srcRoot = resolve(cliRoot, "src");
const obsoleteResolver = ["parse", "Common", "Host", "Flags"].join("");

function typescriptFiles(root: string): string[] {
  const files: string[] = [];
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    const path = resolve(root, entry.name);
    if (entry.isDirectory()) {
      files.push(...typescriptFiles(path));
    } else if (/\.[cm]?tsx?$/.test(entry.name)) {
      files.push(path);
    }
  }
  return files;
}

function read(path: string): string {
  return readFileSync(path, "utf8");
}

function exportedHostHandlers(hostIndex: string): Map<string, string> {
  const handlers = new Map<string, string>();
  const reexports = /export\s*\{([\s\S]*?)\}\s*from\s*"\.\/([^".]+)\.js";/g;
  for (const match of hostIndex.matchAll(reexports)) {
    const moduleName = match[2]!;
    for (const raw of match[1]!.split(",")) {
      const name = raw.trim().replace(/^type\s+/, "").split(/\s+as\s+/)[0];
      if (name) handlers.set(name, moduleName);
    }
  }
  return handlers;
}

function dispatchedHandlers(main: string): Map<string, readonly string[]> {
  const handlers = new Map<string, readonly string[]>();
  const switchBody = main.slice(main.indexOf("switch (sub)"));
  const cases = [...switchBody.matchAll(/case "([^"]+)":([\s\S]*?)(?=\n\s*case "|\n\s*default:)/g)];
  for (const match of cases) {
    handlers.set(match[1]!, [...match[2]!.matchAll(/return\s+([A-Za-z0-9_$]+)\(io,/g)].map((hit) => hit[1]!));
  }
  return handlers;
}

describe("CLI host-auth resolver ownership", () => {
  it("contains no declaration, import, export, call, or documentation reference to the obsolete resolver", () => {
    const offenders = [...typescriptFiles(srcRoot), ...typescriptFiles(resolve(cliRoot, "test"))]
      .filter((path) => read(path).includes(obsoleteResolver))
      .map((path) => relative(cliRoot, path).replaceAll("\\", "/"));
    expect(offenders).toEqual([]);
  });

  it("keeps one synchronous common-flag extractor and no synchronous token-required resolver", () => {
    const common = read(resolve(srcRoot, "host", "common.ts"));
    const synchronousCommonFlagFunctions = [...common.matchAll(/export function (\w*CommonHostFlags)\s*\(/g)]
      .map((match) => match[1]);
    expect(synchronousCommonFlagFunctions).toEqual(["extractCommonHostFlags"]);
  });

  it("routes the exhaustive 20-verb authenticated matrix through one typed preparation owner", () => {
    const main = read(resolve(srcRoot, "main.ts"));
    const hostIndex = read(resolve(srcRoot, "host", "index.ts"));
    const handlerModules = exportedHostHandlers(hostIndex);
    const dispatch = dispatchedHandlers(main);
    const authenticated = new Map<string, "data" | "control">([
      ["start", "data"],
      ["status", "data"],
      ["deliveries", "data"],
      ["wait", "data"],
      ["events", "data"],
      ["otel", "data"],
      ["tail", "data"],
      ["inspect", "data"],
      ["files", "data"],
      ["download", "data"],
      ["cancel", "data"],
      ["delete", "data"],
      ["delete-asset", "data"],
      ["sessions", "data"],
      ["whoami", "data"],
      ["billing", "data"],
      ["webhooks", "data"],
      ["orgs", "control"],
      ["workspaces", "control"],
      ["keys", "control"]
    ]);
    const authOnly = new Set(["login", "logout", "auth"]);
    const unauthenticated = new Set(["models", "providers", "tools", "runtime-sizes"]);

    expect([...dispatch.keys()].sort()).toEqual(CLI_VERBS.map((verb) => verb.name).sort());
    expect([...authenticated.values()].filter((policy) => policy === "data")).toHaveLength(17);
    expect([...authenticated.values()].filter((policy) => policy === "control")).toHaveLength(3);
    expect([...authenticated.keys(), ...authOnly, ...unauthenticated].sort()).toEqual(
      CLI_VERBS.map((verb) => verb.name).sort()
    );

    for (const [name, policy] of authenticated) {
      const candidates = (dispatch.get(name) ?? [])
        .map((handler) => ({ handler, moduleName: handlerModules.get(handler) }))
        .filter((candidate): candidate is { handler: string; moduleName: string } => candidate.moduleName !== undefined);
      expect(candidates.length, `${name} must dispatch through the host implementation barrel`).toBeGreaterThan(0);
      for (const candidate of candidates) {
        const source = read(resolve(srcRoot, "host", `${candidate.moduleName}.ts`));
        expect(source, `${name} (${candidate.handler}) must use the preparation owner`).toContain("prepareHostCommand");
        expect(
          source.match(/await\s+prepareHostCommand\(io,\s*argv,/g) ?? [],
          `${name} must prepare exactly once`
        ).toHaveLength(1);
        expect(source, `${name} must keep its explicit auth policy`).toMatch(
          new RegExp(`verb:\\s*["']${name}["'][\\s\\S]{0,80}auth:\\s*["']${policy}["']`)
        );
        expect(source, `${name} may not bypass preparation refusal`).not.toContain("refuseInsideManagedSession");
        expect(source, `${name} may not bypass preparation auth`).not.toMatch(/resolve(?:Common|ControlPlane)HostFlags/);
      }
    }

    for (const name of authOnly) {
      const modules = (dispatch.get(name) ?? []).map((handler) => handlerModules.get(handler));
      expect(modules.every((moduleName) => moduleName !== undefined), name).toBe(true);
      for (const moduleName of modules) {
        const source = read(resolve(srcRoot, "host", `${moduleName}.ts`));
        expect(source, `${name} keeps its no-auth extractor`).toContain("extractCommonHostFlags");
        expect(source, `${name} remains host-only`).toContain("refuseInsideManagedSession");
        expect(source, `${name} is not an authenticated host command`).not.toContain("prepareHostCommand");
      }
    }

    for (const name of unauthenticated) {
      const modules = (dispatch.get(name) ?? []).map((handler) => handlerModules.get(handler));
      expect(modules.every((moduleName) => moduleName !== undefined), name).toBe(true);
      for (const moduleName of modules) {
        const source = read(resolve(srcRoot, "host", `${moduleName}.ts`));
        expect(source, `${name} is an unauthenticated discovery verb`).not.toMatch(
          /prepareHostCommand|refuseInsideManagedSession|resolve(?:Common|ControlPlane)HostFlags/
        );
      }
    }
  });
});
