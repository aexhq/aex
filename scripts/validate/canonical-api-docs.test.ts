import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { relative, resolve } from "node:path";
import { describe, expect, it } from "bun:test";

const repoRoot = resolve(import.meta.dirname, "..", "..");
const sdkDocs = resolve(repoRoot, "packages", "sdk", "docs");
const deletedGuides = [
  "provider-runtime-capabilities.md",
  "session-config.md",
  "session-record.md",
  "webhooks.md",
  "concepts/providers-and-runtimes.md",
  "concepts/subagents.md"
] as const;

describe("canonical strict-v1 package documentation", () => {
  it("removes guides whose only subject was a deleted public surface", () => {
    for (const guide of deletedGuides) {
      expect(existsSync(resolve(sdkDocs, guide)), guide).toBe(false);
    }
  });

  it("teaches explicit sessions, durable runs and the separate CLI package", () => {
    const root = readFileSync(resolve(repoRoot, "README.md"), "utf8");
    const sdk = readFileSync(resolve(repoRoot, "packages/sdk/README.md"), "utf8");
    const cli = readFileSync(resolve(repoRoot, "packages/cli/README.md"), "utf8");
    const quickstart = readFileSync(resolve(sdkDocs, "quickstart.md"), "utf8");

    for (const text of [root, sdk, quickstart]) {
      expect(text).toContain("aex.sessions.create");
      expect(text).toContain("session.messages.send");
      expect(text).toContain("run.result");
    }
    expect(root).toContain("@aexhq/cli");
    expect(sdk).toContain("published separately by `@aexhq/cli`");
    expect(cli).toContain("sessions create");
    expect(root).not.toContain("npx aex start");
    expect(root).not.toContain("Bundles the CLI");
  });

  it("documents overwrite registries, explicit file grants, and telemetry resources", () => {
    const resources = readFileSync(resolve(sdkDocs, "resources.md"), "utf8");
    const files = readFileSync(resolve(sdkDocs, "files.md"), "utf8");
    const telemetry = readFileSync(resolve(sdkDocs, "telemetry.md"), "utf8");

    expect(resources).toContain("aex.workspace.instructions.set");
    expect(resources).toContain("overwrite");
    expect(resources).not.toContain("versions");
    expect(files).toContain("session.files.persisted.download");
    expect(files).toContain("session.files.live.download");
    expect(files).toContain("short-lived grant");
    expect(telemetry).toContain("session.telemetry.query");
    expect(telemetry).toContain("session.telemetry.stream");
    expect(telemetry).toContain("session.telemetry.export");
  });

  it("contains no stale code examples for removed SDK and wire concepts", () => {
    const findings: string[] = [];
    const forbidden = [
      /\bAssetRef\b/,
      /\bSDK_VERSION\b/,
      /\bavailableRuntimeKinds\b/,
      /\bcanonicalSha256\b/,
      /\bFile\.from(?:Path|Url|Bytes)\b/,
      /\bSkill\.from(?:Dir|Url)\b/,
      /\bTool\.from(?:Dir|Url)\b/,
      /\bruntime\s*:/,
      /\.archiveLink\s*\(/,
      /\.webhooks\b/,
      /\bcheckpointId\b/,
      /\bFargate\b/
    ];
    for (const file of filesUnder(sdkDocs)) {
      for (const [index, line] of readFileSync(file, "utf8").split(/\r?\n/).entries()) {
        if (forbidden.some((pattern) => pattern.test(line))) {
          findings.push(`${relative(repoRoot, file).replaceAll("\\", "/")}:${index + 1}: ${line.trim()}`);
        }
      }
    }
    expect(findings).toEqual([]);
  });
});

function filesUnder(path: string): string[] {
  if (statSync(path).isFile()) return [path];
  return readdirSync(path, { withFileTypes: true }).flatMap((entry) => {
    const child = resolve(path, entry.name);
    if (entry.isDirectory()) return filesUnder(child);
    return entry.isFile() && /\.(?:md|json)$/.test(entry.name) ? [child] : [];
  });
}
