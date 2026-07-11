/**
 * Live scenario: live-sdk-download-namespaces.test.ts
 *
 * Exercises the session-artifact namespace split end-to-end against a real
 * run on the live API:
 *
 *   - A session's deliverables live in the `files` namespace.
 *   - `listFiles` returns ONLY deliverables (no diagnostic-prefixed
 *     entries leak in).
 *   - Public download verbs (`download`, `downloadFiles`) each
 *     produce a valid (PK-magic) zip against the real server.
 *
 * The zip's internal folder layout is pinned by the shared unit test
 * (packages/contracts/test/operations-download.test.ts); here we prove the
 * REAL server keeps diagnostics out of public files and every public
 * download verb round-trips against a live run, without unzipping in the child.
 *
 * Required env: same as the other live-sdk-* files
 * (AEX_API_URL, AEX_API_KEY,
 * DEEPSEEK_API_KEY, + AEX_USER_TEST_TARBALL/VERSION).
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (download-namespaces): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiKey = requireEnv("AEX_API_KEY");
const deepseekKey = requireEnv("DEEPSEEK_API_KEY");
const deepseekModel = process.env["AEX_USER_TEST_DEEPSEEK_MODEL"]?.trim() || "deepseek-v4-flash";

interface Cell {
  readonly id: string;
}

const CELLS: readonly Cell[] = [
  { id: "deepseek-managed-a" }
];

const DIAGNOSTIC_PREFIXES = ["runtime/", "host/"];
const isDiagnostic = (name: string | null): boolean =>
  !!name && DIAGNOSTIC_PREFIXES.some((p) => name.startsWith(p));

function buildPassEnv(extras: Record<string, string>): Record<string, string> {
  const env: Record<string, string> = { ...extras };
  const pathKey = process.platform === "win32" ? "Path" : "PATH";
  if (process.env[pathKey]) env[pathKey] = process.env[pathKey]!;
  if (process.platform === "win32") {
    for (const k of [
      "SystemRoot",
      "SystemDrive",
      "TEMP",
      "TMP",
      "USERPROFILE",
      "APPDATA",
      "LOCALAPPDATA",
      "ComSpec",
      "ProgramFiles",
      "ProgramData"
    ]) {
      if (process.env[k]) env[k] = process.env[k]!;
    }
  }
  return env;
}

interface ZipProbe {
  readonly byteLength: number;
  readonly magicOk: boolean;
}

interface CaseResult {
  readonly sessionId: string;
  readonly runStatus: string;
  readonly files: ReadonlyArray<{ id: string; filename: string | null }>;
  readonly download: ZipProbe;
  readonly downloadFiles: ZipProbe;
  readonly marker: string;
}

function buildScript(cell: Cell, marker: string): string {
  const prompt =
    `Use your filesystem tools to create a file called \`report.txt\` ` +
    `inside \`/workspace/files/report-folder/\`. ` +
    `The file's only contents must be the literal text: ${marker} ` +
    `(no newline, no extra characters). Then reply briefly that you wrote it.`;
  return `
    import { Aex } from "@aexhq/sdk";

    const client = new Aex({
      baseUrl: process.env.AEX_API_URL,
      apiKey: process.env.AEX_API_KEY
    });

    const result = await client.start({
      provider: "deepseek",
      model: ${JSON.stringify(deepseekModel)},
      message: ${JSON.stringify(prompt)},
      builtinTools: "default",
      fileCapture: { allowedDirs: ["/workspace/files/report-folder"] },
      apiKeys: { deepseek: process.env.DEEPSEEK_KEY_SUBMIT },
      idempotencyKey: "dl-namespaces-${cell.id}-" + Date.now()
    }, { timeoutMs: 6 * 60_000 });
    const sessionId = result.sessionId;
    const run = {
      status: result.status,
      runtime: "managed",
      provider: "deepseek"
    };
    const session = await client.sessions.open(sessionId);

    const probe = (bytes) => ({
      byteLength: bytes.byteLength,
      magicOk: bytes.byteLength >= 4 && bytes[0] === 0x50 && bytes[1] === 0x4b && bytes[2] === 0x03 && bytes[3] === 0x04
    });

    const files = (await session.files.list()).files;
    const downloadAll = await session.download();
    const downloadOut = await session.files.download(undefined);

    const payload = {
      sessionId: sessionId,
      runStatus: run.status,
      files: files.map((o) => ({ id: o.id, filename: o.filename ?? null })),
      download: probe(downloadAll),
      downloadFiles: probe(downloadOut),
      marker: ${JSON.stringify(marker)}
    };
    process.stdout.write(JSON.stringify(payload));
    process.exit(0);
  `;
}

function dump(cell: Cell, r: CaseResult): string {
  return [
    `cell=${cell.id} sessionId=${r.sessionId} status=${r.runStatus} marker=${r.marker}`,
    `files=${JSON.stringify(r.files)}`,
    `zips: download=${JSON.stringify(r.download)} files=${JSON.stringify(r.downloadFiles)}`
  ].join("\n");
}

async function runDownloadNamespacesChild(
  install: InstallResult,
  scriptPath: string,
  cell: Cell
): Promise<CaseResult> {
  const child = await runCommand(getBunCommand(), [scriptPath], {
    cwd: install.installDir,
    timeoutMs: 8 * 60_000,
    env: buildPassEnv({
      AEX_API_URL: apiUrl,
      AEX_API_KEY: apiKey,
      DEEPSEEK_KEY_SUBMIT: deepseekKey
    })
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `download-namespaces runner (${cell.id}) exited ${child.exitCode}:\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  return JSON.parse(child.stdout.trim()) as CaseResult;
}

describe("live: session-artifact public files + download verbs", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex();
  });

  afterAll(() => {
    install?.cleanup();
  });

  for (const cell of CELLS) {
    it(
      `[${cell.id}] keeps diagnostics out of files and public download verbs round-trip`,
      async () => {
        const marker = `DLNS-${Math.random().toString(36).slice(2, 10).toUpperCase()}-EOF`;
        const scriptPath = join(install.installDir, `dl-namespaces-${cell.id}.mjs`);
        writeFileSync(scriptPath, buildScript(cell, marker));
        const r = await runDownloadNamespacesChild(install, scriptPath, cell);
        const ctx = `\n\n${dump(cell, r)}`;

        // 1. The `files` namespace is deliverables-only — no diagnostic
        //    artifact leaks into the customer-facing listing.
        const leaked = r.files.filter((o) => isDiagnostic(o.filename));
        expect(leaked, `diagnostics leaked into files listing${ctx}`).toEqual([]);

        // 2. Files have a stable id-space and contain only deliverables.
        const outIds = new Set(r.files.map((o) => o.id));
        expect(outIds.size).toBe(r.files.length);

        // 3. Every public download verb produced a valid (PK-magic) zip.
        for (const [verb, z] of [
          ["download", r.download],
          ["downloadFiles", r.downloadFiles]
        ] as const) {
          expect(z.byteLength, `${verb} zip empty${ctx}`).toBeGreaterThan(0);
          expect(z.magicOk, `${verb} zip not a zip (bad magic)${ctx}`).toBe(true);
        }

        // 4. The session reaches a successful terminal state and the deliverable
        //    is captured in the files namespace. (Asserted unconditionally:
        //    this is the happy path, and checks 1-5 above already assume a
        //    completed run.)
        expect(r.runStatus, `run did not succeed${ctx}`).toBe("succeeded");
        expect(
          r.files.some((o) => (o.filename ?? "").endsWith("report.txt")),
          `report.txt missing from files on a succeeded run${ctx}`
        ).toBe(true);
      },
      9 * 60_000
    );
  }
});
