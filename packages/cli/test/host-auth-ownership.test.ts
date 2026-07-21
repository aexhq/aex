import { readFileSync, readdirSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
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

  it("routes every registered authenticated verb through its intended live resolver", () => {
    const main = read(resolve(srcRoot, "main.ts"));
    const hostIndex = read(resolve(srcRoot, "host", "index.ts"));
    const handlerModules = exportedHostHandlers(hostIndex);
    const dispatch = dispatchedHandlers(main);
    const controlPlane = new Set(["orgs", "workspaces", "keys"]);
    const authOnly = new Set(["login", "logout", "auth"]);
    const unauthenticated = new Set(["models", "providers", "tools", "runtime-sizes"]);

    expect([...dispatch.keys()].sort()).toEqual(CLI_VERBS.map((verb) => verb.name).sort());

    for (const { name } of CLI_VERBS) {
      const candidates = (dispatch.get(name) ?? [])
        .map((handler) => ({ handler, moduleName: handlerModules.get(handler) }))
        .filter((candidate): candidate is { handler: string; moduleName: string } => candidate.moduleName !== undefined);
      expect(candidates.length, `${name} must dispatch through the host implementation barrel`).toBeGreaterThan(0);

      const expectedResolver = controlPlane.has(name)
        ? "resolveControlPlaneHostFlags"
        : authOnly.has(name)
          ? "extractCommonHostFlags"
          : unauthenticated.has(name)
            ? null
            : "resolveCommonHostFlags";
      for (const candidate of candidates) {
        const source = read(resolve(srcRoot, "host", `${candidate.moduleName}.ts`));
        if (expectedResolver === null) {
          expect(source, `${name} is an unauthenticated discovery verb`).not.toMatch(/resolve(?:Common|ControlPlane)HostFlags/);
          continue;
        }
        expect(source, `${name} (${candidate.handler}) must use ${expectedResolver}`).toContain(expectedResolver);
        if (expectedResolver.startsWith("resolve")) {
          const calls = source.match(new RegExp(`await\\s+${expectedResolver}\\(io,\\s*argv\\)`, "g")) ?? [];
          expect(calls, `${name} must resolve host auth exactly once`).toHaveLength(1);
        }
      }
    }
  });
});
