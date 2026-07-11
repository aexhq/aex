/**
 * Live edge-case sweep: retained-storage visibility on the public session record.
 *
 * A session that retains captured files bills retained-file storage (the
 * hourly accrual sweep charges byte-hours from the RUN-terminal byte total).
 * The ONLY public surface where a customer can see the retained byte total is
 * the session record's `retainedStorageBytes` field — and on the dev plane it
 * reads 0 forever: the field is written once at create (hardcoded 0) and no
 * code path (finish, capture, hourly reconcile) ever updates it, while the
 * real total flows only to internal billing storage. Customers are billed for
 * storage they cannot see.
 *
 * ONE billable session turn total: produce a single session file with known contents,
 * wait for RUN_FINISHED, then read a non-zero
 * `retainedStorageBytes`.
 *
 * Required env: AEX_API_URL, AEX_API_KEY, DEEPSEEK_API_KEY, +
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
    throw new Error(`user-tests live (edge-storage-accrual): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiKey = requireEnv("AEX_API_KEY");
const providerKey = requireGateKey("edge-storage-accrual");
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
  const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiKey: process.env.AEX_API_KEY });
  const PROVIDER = process.env.PROVIDER;
  const PROVIDER_KEY = process.env.PROVIDER_KEY;
  const MODEL = process.env.MODEL;
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
      AEX_API_KEY: apiKey,
      PROVIDER: GATE_PROVIDER,
      PROVIDER_KEY: providerKey,
      MODEL: model
    })
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `edge-storage-accrual runner (${scriptName}) exited ${child.exitCode}:\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  try {
    return JSON.parse(child.stdout.trim()) as Record<string, unknown>;
  } catch {
    throw new Error(`edge-storage-accrual runner (${scriptName}) produced non-JSON stdout:\n${child.stdout}`);
  }
}

let install: InstallResult;
beforeAll(async () => {
  install = await installAex();
}, 240_000);
afterAll(() => {
  install?.cleanup();
});

describe("edge: retained-storage visibility (retainedStorageBytes)", () => {
  it(
    "a finished session retaining a captured file surfaces a non-zero retainedStorageBytes on the public session record",
    async () => {
      const body = `
        const prompt =
          "Use your shell tool to write exactly 2048 bytes to /workspace/files/blob.bin " +
          "(for example: head -c 2048 /dev/zero > /workspace/files/blob.bin). " +
          "Create no other files. Then reply with the single word done.";
        const sessionResult = await client.start({
          provider: PROVIDER,
          model: MODEL,
          message: prompt,
          builtinTools: "default",
          fileCapture: { allowedDirs: ["/workspace/files"] },
          apiKeys: { [PROVIDER]: PROVIDER_KEY },
          idempotencyKey: "edge-storage-" + Date.now()
        }, { timeoutMs: 6 * 60_000 });

        const session = await client.sessions.open(sessionResult.sessionId);
        const listed = await session.files.list();
        const blob = listed.files.find((o) => (o.filename || "").endsWith("blob.bin")) || null;
        const retained = session.record.retainedStorageBytes ?? 0;

        process.stdout.write(JSON.stringify({
          sessionId: sessionResult.sessionId,
          ok: sessionResult.ok,
          status: sessionResult.status,
          fileCount: listed.files.length,
          blobBytes: blob ? blob.sizeBytes ?? null : null,
          retainedStorageBytes: retained,
          checkpointId: listed.revision.checkpointId
        }));
        process.exit(0);
      `;
      const out = (await runChild(install, "edge-storage-A.mjs", body)) as {
        sessionId: string;
        ok: boolean;
        status: string;
        fileCount: number;
        blobBytes: number | null;
        retainedStorageBytes: number;
        checkpointId: string;
      };

      // The session itself must have produced and retained the file.
      expect(out.ok, `run ${out.sessionId} did not complete ok (status=${out.status})`).toBe(true);
      expect(
        (out.blobBytes ?? 0) > 0,
        `blob.bin missing or empty in captured files (count=${out.fileCount}, bytes=${out.blobBytes})`
      ).toBe(true);

      // DEFECT PROBE: retained-file storage is billed hourly from the
      // RUN-terminal byte total, but the public record's
      // retainedStorageBytes is written once as 0 at create and never
      // updated by any later code path — customers pay for retained bytes
      // they cannot see on any public surface. This assertion states the
      // contractually sensible behavior: RUN_FINISHED is not emitted until the
      // captured-file byte total is reflected by the read API.
      expect(
        out.retainedStorageBytes,
        `session ${out.sessionId} retains ${out.blobBytes ?? "?"} bytes of captured files after RUN_FINISHED ` +
          `(checkpoint ${out.checkpointId}) but the public session record still reports ` +
          `retainedStorageBytes=0 — retained storage is billed yet invisible to the customer`
      ).toBeGreaterThan(0);
    },
    12 * 60_000
  );
});
