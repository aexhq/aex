import { cpSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { afterAll, describe, expect, it } from "bun:test";

import {
  applyModuleCanary,
  moduleNode,
  moduleSourceTag,
  moduleUpstreamCanaryVersions,
  moduleUpstreamVersions,
  resolveModuleCanaryVersion,
  verifyPackedModuleManifest
} from "../cicd/module-canary.mjs";

const repoRoot = resolve(import.meta.dir, "..", "..");
const SHA = "0123456789abcdef0123456789abcdef01234567";
const RUN = "19876543210";
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
  it("derives the canary from that module's own base version, sha and run", () => {
    for (const id of ["sdk", "contracts", "cli"]) {
      const node = moduleNode(repoRoot, id);
      const version = resolveModuleCanaryVersion(repoRoot, id, SHA, RUN);
      expect(version, id).toBe(`${node.version.replace(/-.*$/, "")}-canary.${RUN}.g${SHA.slice(0, 7)}`);
    }
  });

  it("gives two commits at one base version two distinct versions, per module", () => {
    // The whole point of the scheme. `@aexhq/cli` and `@aexhq/contracts` sit at
    // one base with no canary ever published; under the old scheme each would
    // have got exactly one green publish and a red on the next push.
    const other = "89abcdef0123456789abcdef0123456789abcdef";
    for (const id of ["sdk", "contracts", "cli"]) {
      expect(resolveModuleCanaryVersion(repoRoot, id, SHA, "1"), id).not.toBe(
        resolveModuleCanaryVersion(repoRoot, id, other, "2")
      );
    }
  });

  it("refuses to resolve a version for a private module", () => {
    for (const id of ["docs", "user-tests"]) {
      expect(() => resolveModuleCanaryVersion(repoRoot, id, SHA, RUN), id).toThrow(
        /private and must not be published/
      );
    }
  });

  it("refuses an unknown module and an invalid source sha", () => {
    expect(() => resolveModuleCanaryVersion(repoRoot, "ghost", SHA, RUN)).toThrow(/unknown public module/);
    expect(() => resolveModuleCanaryVersion(repoRoot, "sdk", "not-a-sha", RUN)).toThrow(/invalid source SHA/);
    expect(() => resolveModuleCanaryVersion(repoRoot, "sdk", SHA, "")).toThrow(/run id/);
  });
});

describe("source tags are immutable and cannot collide across modules", () => {
  it("keeps the SDK on the flat tag the private release parses", () => {
    // platform/.github/workflows/deploy-dev.yml:83 and deploy-prd.yml:99 strip the
    // `canary/` prefix to recover the SDK version, so this shape is load-bearing.
    expect(moduleSourceTag("sdk", "0.46.4-canary.7.gabcdef0")).toBe("canary/0.46.4-canary.7.gabcdef0");
  });

  it("namespaces every other module so two modules cannot claim one tag", () => {
    expect(moduleSourceTag("contracts", "0.34.0-canary.7.gabcdef0")).toBe(
      "canary/contracts/0.34.0-canary.7.gabcdef0"
    );
    expect(moduleSourceTag("cli", "0.34.0-canary.7.gabcdef0")).toBe("canary/cli/0.34.0-canary.7.gabcdef0");
    const tags = new Set(["sdk", "contracts", "cli"].map((id) => moduleSourceTag(id, "1.2.3-canary.7.gabcdef0")));
    expect(tags.size).toBe(3);
  });

  it("refuses to name a tag after a non-canary version", () => {
    expect(() => moduleSourceTag("sdk", "0.46.4")).toThrow(/invalid canary version/);
    expect(() => moduleSourceTag("sdk", "latest")).toThrow(/invalid canary version/);
    // The OLD shape too: a tag named after it would pin an unpublishable version.
    expect(() => moduleSourceTag("sdk", "0.46.4-canary")).toThrow(/invalid canary version/);
  });
});

describe("applying a canary binds the package to the exact tested source", () => {
  it("writes version and source sha for a generic module", () => {
    const root = workspaceCopy();
    applyModuleCanary(root, "contracts", { version: "0.34.0-canary.7.gabcdef0", sha: SHA, run: "7" });
    const manifest = manifestAt(root, "packages/contracts/package.json");
    expect(manifest.version).toBe("0.34.0-canary.7.gabcdef0");
    expect(manifest.aexRelease).toEqual({ sourceSha: SHA, upstream: {} });
  });

  it("also rewrites the SDK's exported version constant", () => {
    // A generic package.json writer would leave `SDK_VERSION` stale, so the shipped
    // SDK would report a version it is not.
    const root = workspaceCopy();
    const applied = applyModuleCanary(root, "sdk", {
      version: "0.46.4-canary.7.gabcdef0",
      sha: SHA,
      run: "7"
    });
    expect(manifestAt(root, "packages/sdk/package.json").version).toBe("0.46.4-canary.7.gabcdef0");
    expect(manifestAt(root, "packages/sdk/package.json").aexRelease).toEqual({
      sourceSha: SHA,
      upstream: applied.upstream
    });
    expect(manifestAt(root, "packages/sdk/package.json").devDependencies).toMatchObject(applied.upstream);
    expect(readFileSync(resolve(root, "packages/sdk/src/version.ts"), "utf8")).toContain(
      'export const SDK_VERSION = "0.46.4-canary.7.gabcdef0";'
    );
  });

  it("refuses a mutable version, a bad sha, and a private module", () => {
    const root = workspaceCopy();
    expect(() => applyModuleCanary(root, "cli", { version: "0.34.0", sha: SHA, run: "7" })).toThrow(
      /invalid canary version/
    );
    expect(() => applyModuleCanary(root, "cli", { version: "0.34.0-canary", sha: SHA, run: "7" })).toThrow(
      /invalid canary version/
    );
    expect(() =>
      applyModuleCanary(root, "cli", { version: "0.34.0-canary.7.gabcdef0", sha: "abc", run: "7" })
    ).toThrow(/invalid release source SHA/);
    expect(() =>
      applyModuleCanary(root, "docs", { version: "0.1.0-canary.7.gabcdef0", sha: SHA, run: "7" })
    ).toThrow(/private and must not be published/);
  });
});

describe("upstream package versions are recorded as release identity", () => {
  it("lists only publishable upstream modules", () => {
    // Both embedded artifacts, because both change what the published SDK IS:
    // contracts is inlined into dist/_contracts and the CLI bundle becomes the
    // `aex` bin.
    expect(moduleUpstreamVersions(repoRoot, "sdk")).toEqual({
      "@aexhq/cli": moduleNode(repoRoot, "cli").version,
      "@aexhq/contracts": moduleNode(repoRoot, "contracts").version
    });
    expect(moduleUpstreamVersions(repoRoot, "contracts")).toEqual({});
  });

  it("rewrites and verifies the exact same-run upstream identity in packed metadata", () => {
    const root = workspaceCopy();
    const version = "0.34.0-canary.7.gabcdef0";
    const applied = applyModuleCanary(root, "cli", { version, sha: SHA, run: "7" });
    const packed = manifestAt(root, "packages/cli/package.json");
    const expected = moduleUpstreamCanaryVersions(repoRoot, "cli", SHA, "7");

    expect(applied.upstream).toEqual(expected);
    expect((packed.dependencies as Record<string, string>)["@aexhq/contracts"]).toBe(
      expected["@aexhq/contracts"]
    );
    expect(moduleUpstreamVersions(root, "cli")).toEqual(expected);
    expect(verifyPackedModuleManifest(root, "cli", packed, { version, sha: SHA, run: "7" })).toEqual(expected);

    (packed.dependencies as Record<string, string>)["@aexhq/contracts"] = "workspace:*";
    expect(() => verifyPackedModuleManifest(root, "cli", packed, { version, sha: SHA, run: "7" })).toThrow(
      /dependencies.@aexhq\/contracts/
    );
  });
});
