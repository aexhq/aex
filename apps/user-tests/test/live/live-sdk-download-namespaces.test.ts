/**
 * Live scenario: live-sdk-download-namespaces.test.ts
 *
 * Exercises the run-artifact namespace split end-to-end against a real
 * run on the live API:
 *
 *   - A run's deliverables live in the `outputs` namespace; platform
 *     diagnostics (runtime/, host/) live in the
 *     physically-separate `logs` namespace.
 *   - `listOutputs` returns ONLY deliverables (no diagnostic-prefixed
 *     entries leak in).
 *   - `getRunDebugLogs` returns ONLY diagnostics, dot-stripped
 *     (`runtime/...`, not `.runtime/...`).
 *   - The four download verbs (`download`, `downloadOutputs`,
 *     `downloadLogs`, `downloadEvents` via the everything zip) each
 *     produce a valid (PK-magic) zip against the real server.
 *
 * The zip's internal folder layout is pinned by the shared unit test
 * (packages/contracts/test/operations-download.test.ts); here we prove the
 * REAL server's outputs-vs-logs separation + that every verb round-trips
 * against a live run, without unzipping in the child.
 *
 * Required env: same as the other live-sdk-* files
 * (AEX_API_URL, AEX_API_TOKEN,
 * DEEPSEEK_API_KEY, + AEX_USER_TEST_TARBALL/VERSION).
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (download-namespaces): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiToken = requireEnv("AEX_API_TOKEN");
const deepseekKey = requireEnv("DEEPSEEK_API_KEY");
const deepseekModel = process.env["AEX_USER_TEST_DEEPSEEK_MODEL"] ?? "deepseek-chat";

interface Cell {
  readonly id: string;
  readonly runtime: "managed";
  /** The diagnostic prefix this runtime is guaranteed to emit under logs/. */
  readonly expectedLogPrefix: string;
}

// One managed Anthropic cell pins the diagnostic namespace:
//   managed → runtime/{stdout,stderr,args} (always uploaded)
const CELLS: readonly Cell[] = [
  { id: "deepseek-managed-a", runtime: "managed", expectedLogPrefix: "runtime/" }
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
  readonly runId: string;
  readonly runStatus: string;
  readonly outputs: ReadonlyArray<{ id: string; filename: string | null }>;
  readonly debugLogs: ReadonlyArray<{ filename: string }>;
  readonly download: ZipProbe;
  readonly downloadOutputs: ZipProbe;
  readonly downloadLogs: ZipProbe;
  readonly marker: string;
}

function buildScript(cell: Cell, marker: string): string {
  const prompt =
    `Use your filesystem tools to create a file called \`report.txt\` ` +
    `inside the output directory at \`/workspace/outputs/report-folder/\`. ` +
    `The file's only contents must be the literal text: ${marker} ` +
    `(no newline, no extra characters). Then reply briefly that you wrote it.`;
  return `
    import { AexClient } from "@aexhq/sdk";

    const client = new AexClient({
      baseUrl: process.env.AEX_API_URL,
      apiToken: process.env.AEX_API_TOKEN
    });

    const runId = await client.submitRun({
      provider: "deepseek",
      runtime: ${JSON.stringify(cell.runtime)},
      model: ${JSON.stringify(deepseekModel)},
      prompt: ${JSON.stringify(prompt)},
      builtins: ["developer"],
      outputDirs: ["/workspace/outputs/report-folder"],
      secrets: { deepseek: { apiKey: process.env.DEEPSEEK_KEY_SUBMIT } },
      idempotencyKey: "dl-namespaces-${cell.id}-" + Date.now()
    });

    const deadline = Date.now() + 6 * 60_000;
    let run = null;
    while (Date.now() < deadline) {
      run = await client.getRun(runId);
      if (run.status === "succeeded" || run.status === "failed" || run.status === "cancelled") break;
      await new Promise((r) => setTimeout(r, 2_500));
    }
    if (!run || (run.status !== "succeeded" && run.status !== "failed" && run.status !== "cancelled")) {
      process.stderr.write(JSON.stringify({ kind: "timeout", run }, null, 2));
      process.exit(2);
    }

    const probe = (bytes) => ({
      byteLength: bytes.byteLength,
      magicOk: bytes.byteLength >= 4 && bytes[0] === 0x50 && bytes[1] === 0x4b && bytes[2] === 0x03 && bytes[3] === 0x04
    });

    const outputs = await client.listOutputs(runId);
    const debug = await client.getRunDebugLogs(runId);
    const downloadAll = await client.download(runId);
    const downloadOut = await client.downloadOutputs(runId);
    const downloadLog = await client.downloadLogs(runId);

    const result = {
      runId: runId,
      runStatus: run.status,
      outputs: outputs.map((o) => ({ id: o.id, filename: o.filename ?? null })),
      debugLogs: debug.logs.map((l) => ({ filename: l.filename })),
      download: probe(downloadAll),
      downloadOutputs: probe(downloadOut),
      downloadLogs: probe(downloadLog),
      marker: ${JSON.stringify(marker)}
    };
    process.stdout.write(JSON.stringify(result));
  `;
}

function dump(cell: Cell, r: CaseResult): string {
  return [
    `cell=${cell.id} runId=${r.runId} status=${r.runStatus} marker=${r.marker}`,
    `outputs=${JSON.stringify(r.outputs)}`,
    `debugLogs=${JSON.stringify(r.debugLogs)}`,
    `zips: download=${JSON.stringify(r.download)} outputs=${JSON.stringify(r.downloadOutputs)} logs=${JSON.stringify(r.downloadLogs)}`
  ].join("\n");
}

describe("live: run-artifact namespaces (outputs vs logs) + download verbs", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex();
  });

  afterAll(() => {
    install?.cleanup();
  });

  for (const cell of CELLS) {
    it(`[${cell.id}] splits deliverables from diagnostics and every download verb round-trips`, async () => {
      const marker = `DLNS-${Math.random().toString(36).slice(2, 10).toUpperCase()}-EOF`;
      const scriptPath = join(install.installDir, `dl-namespaces-${cell.id}.mjs`);
      writeFileSync(scriptPath, buildScript(cell, marker));
      const child = await runCommand(process.execPath, [scriptPath], {
        cwd: install.installDir,
        timeoutMs: 8 * 60_000,
        env: buildPassEnv({
          AEX_API_URL: apiUrl,
          AEX_API_TOKEN: apiToken,
          DEEPSEEK_KEY_SUBMIT: deepseekKey
        })
      });
      if (child.exitCode !== 0) {
        throw new Error(
          `download-namespaces runner (${cell.id}) exited ${child.exitCode}:\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
        );
      }
      const r = JSON.parse(child.stdout.trim()) as CaseResult;
      const ctx = `\n\n${dump(cell, r)}`;

      // 1. The `outputs` namespace is deliverables-only — no diagnostic
      //    artifact leaks into the customer-facing listing.
      const leaked = r.outputs.filter((o) => isDiagnostic(o.filename));
      expect(leaked, `diagnostics leaked into outputs listing${ctx}`).toEqual([]);

      // 2. The `logs` namespace (via getRunDebugLogs) is ALL diagnostics,
      //    and dot-stripped (e.g. "runtime/…", not ".runtime/…").
      expect(r.debugLogs.length, `expected at least one diagnostic${ctx}`).toBeGreaterThan(0);
      for (const l of r.debugLogs) {
        expect(isDiagnostic(l.filename), `non-diagnostic in logs namespace: ${l.filename}${ctx}`).toBe(true);
        expect(l.filename.startsWith("."), `logs entry not dot-stripped: ${l.filename}${ctx}`).toBe(false);
      }

      // 3. This runtime's guaranteed diagnostic prefix is present.
      expect(
        r.debugLogs.some((l) => l.filename.startsWith(cell.expectedLogPrefix)),
        `expected a ${cell.expectedLogPrefix} artifact${ctx}`
      ).toBe(true);

      // 4. outputs and logs are disjoint id-spaces.
      const outIds = new Set(r.outputs.map((o) => o.id));
      const logNames = new Set(r.debugLogs.map((l) => l.filename));
      for (const o of r.outputs) expect(logNames.has(o.filename ?? "")).toBe(false);
      expect(outIds.size).toBe(r.outputs.length);

      // 5. Every download verb produced a valid (PK-magic) zip.
      for (const [verb, z] of [
        ["download", r.download],
        ["downloadOutputs", r.downloadOutputs],
        ["downloadLogs", r.downloadLogs]
      ] as const) {
        expect(z.byteLength, `${verb} zip empty${ctx}`).toBeGreaterThan(0);
        expect(z.magicOk, `${verb} zip not a zip (bad magic)${ctx}`).toBe(true);
      }

      // 6. The run reaches a successful terminal state and the deliverable
      //    is captured in the outputs namespace. (Asserted unconditionally:
      //    this is the happy path, and checks 1–5 above already assume a
      //    completed run.)
      expect(r.runStatus, `run did not succeed${ctx}`).toBe("succeeded");
      expect(
        r.outputs.some((o) => (o.filename ?? "").endsWith("report.txt")),
        `report.txt missing from outputs on a succeeded run${ctx}`
      ).toBe(true);
    });
  }
});
