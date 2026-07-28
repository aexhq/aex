import { cpSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { afterAll, describe, expect, it } from "bun:test";

import {
  applyModuleCanary,
  moduleNode,
  moduleSourceTag,
  moduleUpstreamVersions,
  resolveModuleCanaryVersion
} from "../cicd/module-canary.mjs";

const repoRoot = resolve(import.meta.dir, "..", "..");
const SHA = "0123456789abcdef0123456789abcdef01234567";
const temporaryRoots: string[] = [];

afterAll(() => {
  for (const root of temporaryRoots) rmSync(root, { recursive: true, force: true });
});

/** A writable copy of the real workspace manifests — apply() rewrites files. */
function workspaceCopy(): string {
  const dir = mkdtempSync(resolve(tmpdir(), "aex-module-canary-"));
  temporaryRoots.push(dir);
  cpSync(resolve(repoRoot, "package.json"), resolve(dir, "package.json"));
  for (const relative of [
    "packages/sdk/package.json",
    "packages/sdk/src/version.ts",
    "packages/contracts/package.json",
    "packages/cli/package.json",
    "apps/docs/package.json",
    "apps/user-tests/package.json"
  ]) {
    cpSync(resolve(repoRoot, relative), resolve(dir, relative), { recursive: true });
  }
  return dir;
}

function manifestAt(root: string, path: string): Record<string, unknown> {
  return JSON.parse(readFileSync(resolve(root, path), "utf8"));
}

describe("every publishable module resolves its own canary version", () => {
  it("derives the canary from that module's own base version", () => {
    for (const id of ["sdk", "contracts", "cli"]) {
      const node = moduleNode(repoRoot, id);
      const version = resolveModuleCanaryVersion(repoRoot, id, SHA);
      expect(version, id).toBe(`${node.version.replace(/-.*$/, "")}-canary`);
    }
  });

  it("refuses to resolve a version for a private module", () => {
    for (const id of ["docs", "user-tests"]) {
      expect(() => resolveModuleCanaryVersion(repoRoot, id, SHA), id).toThrow(/private and must not be published/);
    }
  });

  it("refuses an unknown module and an invalid source sha", () => {
    expect(() => resolveModuleCanaryVersion(repoRoot, "ghost", SHA)).toThrow(/unknown public module/);
    expect(() => resolveModuleCanaryVersion(repoRoot, "sdk", "not-a-sha")).toThrow(/invalid source SHA/);
  });
});

describe("source tags are immutable and cannot collide across modules", () => {
  it("keeps the SDK on the flat tag the private release parses", () => {
    // platform/.github/workflows/deploy-dev.yml:83 and deploy-prd.yml:99 strip the
    // `canary/` prefix to recover the SDK version, so this shape is load-bearing.
    expect(moduleSourceTag("sdk", "0.46.4-canary")).toBe("canary/0.46.4-canary");
  });

  it("namespaces every other module so two modules cannot claim one tag", () => {
    expect(moduleSourceTag("contracts", "0.34.0-canary")).toBe("canary/contracts/0.34.0-canary");
    expect(moduleSourceTag("cli", "0.34.0-canary")).toBe("canary/cli/0.34.0-canary");
    const tags = new Set(["sdk", "contracts", "cli"].map((id) => moduleSourceTag(id, "1.2.3-canary")));
    expect(tags.size).toBe(3);
  });

  it("refuses to name a tag after a non-canary version", () => {
    expect(() => moduleSourceTag("sdk", "0.46.4")).toThrow(/invalid canary version/);
    expect(() => moduleSourceTag("sdk", "latest")).toThrow(/invalid canary version/);
  });
});

describe("applying a canary binds the package to the exact tested source", () => {
  it("writes version and source sha for a generic module", () => {
    const root = workspaceCopy();
    applyModuleCanary(root, "contracts", { version: "0.34.0-canary", sha: SHA });
    const manifest = manifestAt(root, "packages/contracts/package.json");
    expect(manifest.version).toBe("0.34.0-canary");
    expect(manifest.aexRelease).toEqual({ sourceSha: SHA });
  });

  it("also rewrites the SDK's exported version constant", () => {
    // A generic package.json writer would leave `SDK_VERSION` stale, so the shipped
    // SDK would report a version it is not.
    const root = workspaceCopy();
    applyModuleCanary(root, "sdk", { version: "0.46.4-canary", sha: SHA });
    expect(manifestAt(root, "packages/sdk/package.json").version).toBe("0.46.4-canary");
    expect(manifestAt(root, "packages/sdk/package.json").aexRelease).toEqual({ sourceSha: SHA });
    expect(readFileSync(resolve(root, "packages/sdk/src/version.ts"), "utf8")).toContain(
      'export const SDK_VERSION = "0.46.4-canary";'
    );
  });

  it("refuses a mutable version, a bad sha, and a private module", () => {
    const root = workspaceCopy();
    expect(() => applyModuleCanary(root, "cli", { version: "0.34.0", sha: SHA })).toThrow(/invalid canary version/);
    expect(() => applyModuleCanary(root, "cli", { version: "0.34.0-canary", sha: "abc" })).toThrow(
      /invalid release source SHA/
    );
    expect(() => applyModuleCanary(root, "docs", { version: "0.1.0-canary", sha: SHA })).toThrow(
      /private and must not be published/
    );
  });
});

describe("upstream package versions are recorded as release identity", () => {
  it("lists only publishable upstream modules", () => {
    expect(moduleUpstreamVersions(repoRoot, "sdk")).toEqual({
      "@aexhq/contracts": moduleNode(repoRoot, "contracts").version
    });
    expect(moduleUpstreamVersions(repoRoot, "contracts")).toEqual({});
  });
});
