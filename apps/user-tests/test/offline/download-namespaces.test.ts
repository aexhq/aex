/**
 * Offline scenario: download-namespaces.test.ts
 *
 * Verifies the session-artifact download surface ships in the *installed*
 * package — no live API required:
 *
 *   - `SessionHandle` exposes the whole-run verb `download` and the metadata
 *     verb `downloadMetadata` flat, plus the per-namespace download verbs via
 *     its `files.download()` / `events.download()` accessors.
 *   - The installed SDK assembles a public session archive from metadata, events,
 *     and files only; it must not call the removed logs namespace or event
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

  it("SessionHandle exposes the whole-run + per-namespace download verbs", async () => {
    const script = `
      const { Aex } = await import("@aexhq/sdk");
      const fetch = async (input) => {
        const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
        const parsed = new URL(url);
        if (parsed.pathname === "/api/sessions/sess-1") {
          return new Response(JSON.stringify({ session: { id: "sess-1", status: "idle", acceptsMessages: true } }), {
            status: 200,
            headers: { "content-type": "application/json" }
          });
        }
        return new Response(JSON.stringify({ ok: true }), { status: 200, headers: { "content-type": "application/json" } });
      };
      const c = new Aex({ apiKey: "t", baseUrl: "https://example.test", fetch });
      const session = await c.sessions.open("sess-1");
      const result = {
        session: {
          download: typeof session.download,
          downloadMetadata: typeof session.downloadMetadata,
          filesDownload: typeof session.files.download,
          eventsDownload: typeof session.events.download
        },
        // The whole download surface moved off the client onto the session handle.
        client: {
          download: typeof c.download,
          downloadMetadata: typeof c.downloadMetadata
        }
      };
      // The old flat per-namespace verbs folded into the files/events
      // accessors; the removed logs verbs stay gone everywhere.
      for (const removed of ["downloadFiles", "downloadEvents", "downloadLogs", "getLegacyDebugLogs", "debugLogs"]) {
        result.session[removed] = typeof session[removed];
        result.client[removed] = typeof c[removed];
      }
      process.stdout.write(JSON.stringify(result));
    `;
    const path = join(install.installDir, "download-verbs.mjs");
    writeFileSync(path, script);
    const child = await runCommand(getBunCommand(), [path], { cwd: install.installDir, timeoutMs: 30_000 });
    expect(child.exitCode, child.stderr).toBe(0);
    const result = JSON.parse(child.stdout) as { session: Record<string, string>; client: Record<string, string> };
    for (const v of ["download", "downloadMetadata", "filesDownload", "eventsDownload"]) {
      expect(result.session[v], `SessionHandle ${v} should be a function`).toBe("function");
    }
    for (const v of ["download", "downloadMetadata"]) {
      expect(result.client[v], `Aex.${v} should not be exposed`).toBe("undefined");
    }
    for (const removed of ["downloadFiles", "downloadEvents", "downloadLogs", "getLegacyDebugLogs", "debugLogs"]) {
      expect(result.session[removed], `SessionHandle.${removed} should not be exposed`).toBe("undefined");
      expect(result.client[removed], `Aex.${removed} should not be exposed`).toBe("undefined");
    }
  });

  it("SessionHandle.download assembles only public namespaces from the installed SDK", async () => {
    const script = `
      const { Aex } = await import("@aexhq/sdk");
      const { strFromU8, unzipSync } = await import("fflate");
      const { createHash } = await import("node:crypto");
      const reportBytes = new TextEncoder().encode("hello");
      const reportSha256 = createHash("sha256").update(reportBytes).digest("hex");
      const calls = [];
      const fetch = async (input) => {
        const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
        const parsed = new URL(url);
        const key = parsed.pathname + parsed.search;
        if (key.includes("/logs") || key.includes("channel=")) {
          throw new Error("unexpected non-public download route: " + key);
        }
        // Opening the session handle reads the session record; keep it out of
        // the asserted archive-assembly call set.
        if (key === "/api/sessions/session-1") {
          return new Response(JSON.stringify({ session: { id: "session-1", status: "idle", acceptsMessages: true } }), {
            status: 200,
            headers: { "content-type": "application/json" }
          });
        }
        calls.push(key);
        if (key === "/api/sessions/session-1") {
          return new Response(JSON.stringify({ id: "session-1", status: "idle", acceptsMessages: true }), {
            status: 200,
            headers: { "content-type": "application/json" }
          });
        }
        if (key === "/api/sessions/session-1/events") {
          return new Response(JSON.stringify({ events: [{ id: "evt-1", type: "TEXT_MESSAGE_CONTENT" }] }), {
            status: 200,
            headers: { "content-type": "application/json" }
          });
        }
        if (key === "/api/sessions/session-1/files") {
          return new Response(JSON.stringify({
            revision: {
              checkpointId: "cp-1",
              runId: "run-1",
              turnSeq: 1,
              committedAt: "2026-07-10T00:00:00.000Z",
              throughSeq: 2
            },
            files: [{
              id: "out-1",
              checkpointId: "cp-1",
              filename: "report.txt",
              sizeBytes: reportBytes.byteLength,
              sha256: reportSha256,
              contentType: "text/plain"
            }]
          }), {
            status: 200,
            headers: { "content-type": "application/json" }
          });
        }
        if (key.startsWith("/api/sessions/session-1/files/out-1/download?checkpointId=cp-1")) {
          return new Response(reportBytes, { status: 200, headers: { "content-type": "text/plain" } });
        }
        throw new Error("unexpected route: " + key);
      };

      const client = new Aex({ apiKey: "t", baseUrl: "https://example.test", fetch });
      const session = await client.sessions.open("session-1");
      const entries = unzipSync(await session.download());
      const manifest = JSON.parse(strFromU8(entries["manifest.json"]));
      process.stdout.write(JSON.stringify({
        calls,
        entries: Object.keys(entries).sort(),
        manifestNamespaces: manifest.namespaces.map((entry) => entry.name),
        manifestHasLogsAlias: Object.prototype.hasOwnProperty.call(manifest, "logs"),
        outputText: strFromU8(entries["files/report.txt"])
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
      "/api/sessions/session-1/events",
      "/api/sessions/session-1/files",
      "/api/sessions/session-1/files/out-1/download?checkpointId=cp-1"
    ].sort());
    expect(result.entries).toEqual([
      "events/events.jsonl",
      "files/report.txt",
      "manifest.json",
      "metadata/session.json"
    ]);
    expect(result.manifestNamespaces).toEqual(["metadata", "events", "files"]);
    expect(result.manifestHasLogsAlias).toBe(false);
    expect(result.outputText).toBe("hello");
  });

  it("`aex download` usage advertises --only and its namespaces", async () => {
    expect(existsSync(binPath)).toBe(true);
    // No session id → usage error (exit 2) that lists the --only namespaces.
    const result = await runCommand(
      binPath,
      ["download", "--api-key", "t", "--aex-url", "https://example.test"],
      { cwd: install.installDir, timeoutMs: 30_000 }
    );
    expect(result.exitCode).toBe(2);
    expect(result.stderr).toMatch(/--only files\|events\|metadata/);
  });

  it("`aex download --only logs` rejects as a removed public namespace", async () => {
    const result = await runCommand(
      binPath,
      ["download", "session-x", "--only", "logs", "--api-key", "t", "--aex-url", "https://example.test"],
      { cwd: install.installDir, timeoutMs: 30_000 }
    );
    expect(result.exitCode).toBe(2);
    expect(result.stderr).toMatch(/--only must be one of: files, events, metadata/);
  });

  it("`aex download --only <bogus>` rejects before any network call", async () => {
    const result = await runCommand(
      binPath,
      ["download", "session-x", "--only", "bogus", "--api-key", "t", "--aex-url", "https://example.test"],
      { cwd: install.installDir, timeoutMs: 30_000 }
    );
    expect(result.exitCode).toBe(2);
    expect(result.stderr).toMatch(/--only must be one of: files, events, metadata/);
  });
});
