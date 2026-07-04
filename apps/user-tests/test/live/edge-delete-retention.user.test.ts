/**
 * Live edge-case sweep: run deletion must actually retire the run's data.
 *
 * DEFECT PROBE — on the dev plane, DELETE /api/runs/:id (SDK
 * `session.delete()`, CLI `aex delete`) only flips the record's status
 * attribute to "deleted" (api.ts deleteRun): it deletes nothing from the
 * output store, no purge job consumes `runDeletedAt`, and none of the read
 * paths (outputs list / download / link / archive) gate on the deleted
 * status. Consequences a customer can observe:
 *   1. Every output of a "deleted" run stays listable AND downloadable
 *      byte-for-byte, indefinitely.
 *   2. The hourly retained-storage accrual bills the deleted run forever —
 *      its basis (run_cost storedBytes) is never cleared by deletion.
 *
 * This probe covers (1), the public surface: delete a settled run, then
 * assert its output content is no longer retrievable. It FAILS until the
 * platform purges (or at least fences reads of) deleted runs' outputs.
 *
 * ONE billable run total (tiny prompt, one small output file).
 *
 * Required env: AEX_API_URL, AEX_API_TOKEN, DEEPSEEK_API_KEY, +
 * AEX_USER_TEST_TARBALL/VERSION (wired by the shared runner).
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";
import { GATE_PROVIDER, gateModel, requireGateKey } from "../_fixtures/provider.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (edge-delete-retention): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiToken = requireEnv("AEX_API_TOKEN");
const providerKey = requireGateKey("edge-delete-retention");
const model = gateModel();

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
  } else {
    for (const k of ["HOME", "TMPDIR", "LANG", "LC_ALL"]) {
      if (process.env[k]) env[k] = process.env[k]!;
    }
  }
  return env;
}

const CHILD_PRELUDE = `
  import { Aex } from "@aexhq/sdk";
  const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiToken: process.env.AEX_API_TOKEN });
  const PROVIDER = process.env.PROVIDER;
  const PROVIDER_KEY = process.env.PROVIDER_KEY;
  const MODEL = process.env.MODEL;
  const errShape = (e) => ({
    name: e && e.constructor ? e.constructor.name : "Error",
    message: e && e.message ? String(e.message).slice(0, 300) : String(e),
    status: e && typeof e.status === "number" ? e.status : null,
    code: e && typeof e.code === "string" ? e.code : null
  });
`;

async function runChild(
  install: InstallResult,
  scriptName: string,
  body: string,
  timeoutMs = 10 * 60_000
): Promise<Record<string, unknown>> {
  const scriptPath = join(install.installDir, scriptName);
  writeFileSync(scriptPath, `${CHILD_PRELUDE}\n${body}\n`);
  const child = await runCommand(getBunCommand(), [scriptPath], {
    cwd: install.installDir,
    timeoutMs,
    env: buildPassEnv({
      AEX_API_URL: apiUrl,
      AEX_API_TOKEN: apiToken,
      PROVIDER: GATE_PROVIDER,
      PROVIDER_KEY: providerKey,
      MODEL: model
    })
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `edge-delete-retention runner (${scriptName}) exited ${child.exitCode}:\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  try {
    return JSON.parse(child.stdout.trim()) as Record<string, unknown>;
  } catch {
    throw new Error(`edge-delete-retention runner (${scriptName}) produced non-JSON stdout:\n${child.stdout}`);
  }
}

let install: InstallResult;
beforeAll(async () => {
  install = await installAex();
}, 240_000);
afterAll(() => {
  install?.cleanup();
});

interface DeleteRetentionResult {
  readonly runId: string;
  readonly ok: boolean;
  readonly status: string;
  readonly preDeleteBytes: number | null;
  readonly deletedStatus: string | null;
  readonly postDeleteListCount: number | null;
  readonly postDeleteListError: { name: string; message: string; status: number | null; code: string | null } | null;
  readonly postDeleteReadText: string | null;
  readonly postDeleteReadError: { name: string; message: string; status: number | null; code: string | null } | null;
}

describe("edge: deleting a run retires its outputs", () => {
  it(
    "outputs of a deleted run are no longer listable or downloadable",
    async () => {
      const body = `
        const marker = "DELETE-RETENTION-" + Date.now();
        const runResult = await client.run({
          provider: PROVIDER,
          model: MODEL,
          message: "Write a file /workspace/keep.txt containing exactly this line: " + marker + " . Then reply done.",
          includeBuiltinTools: true,
          apiKeys: { [PROVIDER]: PROVIDER_KEY },
          idempotencyKey: "edge-delete-retention-" + Date.now()
        }, { timeoutMs: 6 * 60_000 });

        const session = await client.sessions.open(runResult.runId);
        const outs = session.outputs();
        const listed = await outs.list();
        const pre = listed.find((o) => (o.filename || "").endsWith("keep.txt")) || null;

        await session.delete();
        const deletedStatus = (await client.sessions.open(runResult.runId).then((h) => h.record.status).catch(() => null));

        let postDeleteListCount = null, postDeleteListError = null;
        try {
          postDeleteListCount = (await outs.list()).length;
        } catch (e) { postDeleteListError = errShape(e); }

        let postDeleteReadText = null, postDeleteReadError = null;
        try {
          const read = await outs.read({ path: "keep.txt" });
          postDeleteReadText = typeof read === "string" ? read : (read && typeof read.text === "string" ? read.text : JSON.stringify(read));
        } catch (e) { postDeleteReadError = errShape(e); }

        process.stdout.write(JSON.stringify({
          runId: runResult.runId,
          ok: runResult.ok,
          status: runResult.status,
          preDeleteBytes: pre ? pre.sizeBytes ?? null : null,
          deletedStatus,
          postDeleteListCount,
          postDeleteListError,
          postDeleteReadText: typeof postDeleteReadText === "string" ? postDeleteReadText.slice(0, 100) : postDeleteReadText,
          postDeleteReadError
        }));
        process.exit(0);
      `;
      const out = (await runChild(install, "edge-delete-retention-A.mjs", body)) as unknown as DeleteRetentionResult;

      // The run itself must have completed and captured the output.
      expect(out.ok, `run ${out.runId} did not complete ok (status=${out.status})`).toBe(true);
      expect((out.preDeleteBytes ?? 0) > 0, `keep.txt was not captured pre-delete (${out.runId})`).toBe(true);

      // The delete must have been accepted.
      expect(out.deletedStatus, `run ${out.runId}: status after delete`).toBe("deleted");

      // DEFECT PROBE: after a successful delete the output CONTENT must be
      // unreachable — either the list is empty or the read fails with a typed
      // error. Today the dev plane serves both (soft status flip only), so a
      // customer's "deleted" deliverables remain downloadable forever (and the
      // storage accrual keeps billing them).
      const contentStillServed = out.postDeleteReadText !== null && out.postDeleteReadError === null;
      expect(
        contentStillServed,
        `run ${out.runId}: output content is still downloadable after delete ` +
          `(list count=${out.postDeleteListCount}, read="${out.postDeleteReadText}")`
      ).toBe(false);
    },
    12 * 60_000
  );
});
