/**
 * Offline scenario: download-namespaces.test.ts
 *
 * Verifies the run-artifact download surface ships in the *installed*
 * package — no live API required:
 *
 *   - `AgentExecutor` exposes the whole-run verb `download` plus the public
 *     per-namespace verbs `downloadOutputs` / `downloadEvents` /
 *     `downloadMetadata`.
 *   - The installed SDK assembles a public run archive from metadata, events,
 *     and outputs only; it must not call the removed logs namespace or event
 *     channel opt-in routes.
 *   - The CLI `download` command validates `--only <namespace>` BEFORE any
 *     network call: an unknown namespace exits non-zero with the
 *     documented "must be one of" usage error, and the bare-usage banner
 *     advertises the flag.
 *
 * Every assertion runs against `node_modules/@aexhq/sdk` in a fresh install
 * tempdir, so it exercises the published shape, not the monorepo symlink.
 */
import { existsSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getAexBinPath, getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

describe("download namespaces surface (offline)", () => {
  let install: InstallResult;
  let binPath: string;

  beforeAll(async () => {
    install = await installAex();
    binPath = getAexBinPath(install.installDir);
  });

  afterAll(() => {
    install?.cleanup();
  });

  it("AgentExecutor exposes the whole-run + per-namespace download verbs", async () => {
    const script = `
      const { AgentExecutor } = await import("@aexhq/sdk");
      const c = new AgentExecutor({ apiToken: "t", baseUrl: "https://example.test" });
      const verbs = ["download", "downloadOutputs", "downloadEvents", "downloadMetadata"];
      const result = {};
      for (const v of verbs) result[v] = typeof c[v];
      for (const removed of ["downloadLogs", "getRunDebugLogs", "debugLogs"]) {
        result[removed] = typeof c[removed];
      }
      process.stdout.write(JSON.stringify(result));
    `;
    const path = join(install.installDir, "download-verbs.mjs");
    writeFileSync(path, script);
    const child = await runCommand(getBunCommand(), [path], { cwd: install.installDir, timeoutMs: 30_000 });
    expect(child.exitCode).toBe(0);
    const result = JSON.parse(child.stdout) as Record<string, string>;
    for (const v of ["download", "downloadOutputs", "downloadEvents", "downloadMetadata"]) {
      expect(result[v], `AgentExecutor.${v} should be a function`).toBe("function");
    }
    for (const removed of ["downloadLogs", "getRunDebugLogs", "debugLogs"]) {
      expect(result[removed], `AgentExecutor.${removed} should not be exposed`).toBe("undefined");
    }
  });

  it("AgentExecutor.download assembles only public namespaces from the installed SDK", async () => {
    const script = `
      const { AgentExecutor } = await import("@aexhq/sdk");
      const { strFromU8, unzipSync } = await import("fflate");
      const calls = [];
      const fetch = async (input) => {
        const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
        const parsed = new URL(url);
        const key = parsed.pathname + parsed.search;
        calls.push(key);
        if (key.includes("/logs") || key.includes("channel=")) {
          throw new Error("unexpected non-public download route: " + key);
        }
        if (key === "/api/runs/run-1") {
          return new Response(JSON.stringify({ id: "run-1", status: "succeeded" }), {
            status: 200,
            headers: { "content-type": "application/json" }
          });
        }
        if (key === "/api/runs/run-1/events") {
          return new Response(JSON.stringify({ events: [{ id: "evt-1", type: "TEXT_MESSAGE_CONTENT" }] }), {
            status: 200,
            headers: { "content-type": "application/json" }
          });
        }
        if (key === "/api/runs/run-1/outputs") {
          return new Response(JSON.stringify({
            outputs: [{ id: "out-1", filename: "report.txt", sizeBytes: 5, contentType: "text/plain" }]
          }), {
            status: 200,
            headers: { "content-type": "application/json" }
          });
        }
        if (key === "/api/runs/run-1/outputs/out-1/download") {
          return new Response("hello", { status: 200, headers: { "content-type": "text/plain" } });
        }
        throw new Error("unexpected route: " + key);
      };

      const client = new AgentExecutor({ apiToken: "t", baseUrl: "https://example.test", fetch });
      const entries = unzipSync(await client.download("run-1"));
      const manifest = JSON.parse(strFromU8(entries["manifest.json"]));
      process.stdout.write(JSON.stringify({
        calls,
        entries: Object.keys(entries).sort(),
        manifestNamespaces: manifest.namespaces.map((entry) => entry.name),
        manifestHasLogsAlias: Object.prototype.hasOwnProperty.call(manifest, "logs"),
        outputText: strFromU8(entries["outputs/report.txt"])
      }));
    `;
    const path = join(install.installDir, "download-public-archive.mjs");
    writeFileSync(path, script);
    const child = await runCommand(getBunCommand(), [path], { cwd: install.installDir, timeoutMs: 30_000 });
    expect(child.exitCode, child.stderr).toBe(0);
    const result = JSON.parse(child.stdout) as {
      readonly calls: readonly string[];
      readonly entries: readonly string[];
      readonly manifestNamespaces: readonly string[];
      readonly manifestHasLogsAlias: boolean;
      readonly outputText: string;
    };
    expect([...result.calls].sort()).toEqual([
      "/api/runs/run-1",
      "/api/runs/run-1/events",
      "/api/runs/run-1/outputs",
      "/api/runs/run-1/outputs/out-1/download"
    ].sort());
    expect(result.entries).toEqual([
      "events/events.jsonl",
      "manifest.json",
      "metadata/run.json",
      "outputs/report.txt"
    ]);
    expect(result.manifestNamespaces).toEqual(["metadata", "events", "outputs"]);
    expect(result.manifestHasLogsAlias).toBe(false);
    expect(result.outputText).toBe("hello");
  });

  it("`aex download` usage advertises --only and its namespaces", async () => {
    expect(existsSync(binPath)).toBe(true);
    // No run id → usage error (exit 2) that lists the --only namespaces.
    const result = await runCommand(
      binPath,
      ["download", "--api-token", "t", "--aex-url", "https://example.test"],
      { cwd: install.installDir, timeoutMs: 30_000 }
    );
    expect(result.exitCode).toBe(2);
    expect(result.stderr).toMatch(/--only outputs\|events\|metadata/);
  });

  it("`aex download --only logs` rejects as a removed public namespace", async () => {
    const result = await runCommand(
      binPath,
      ["download", "run-x", "--only", "logs", "--api-token", "t", "--aex-url", "https://example.test"],
      { cwd: install.installDir, timeoutMs: 30_000 }
    );
    expect(result.exitCode).toBe(2);
    expect(result.stderr).toMatch(/--only must be one of: outputs, events, metadata/);
  });

  it("`aex download --only <bogus>` rejects before any network call", async () => {
    const result = await runCommand(
      binPath,
      ["download", "run-x", "--only", "bogus", "--api-token", "t", "--aex-url", "https://example.test"],
      { cwd: install.installDir, timeoutMs: 30_000 }
    );
    expect(result.exitCode).toBe(2);
    expect(result.stderr).toMatch(/--only must be one of: outputs, events, metadata/);
  });
});
