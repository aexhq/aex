/**
 * Live edge-case sweep: retained-storage visibility on the public session record.
 *
 * A session that retains captured outputs bills retained-output storage (the
 * hourly accrual sweep charges byte-hours from the settle-recorded byte total).
 * The ONLY public surface where a customer can see the retained byte total is
 * the session record's `retainedStorageBytes` field — and on the dev plane it
 * reads 0 forever: the field is written once at create (hardcoded 0) and no
 * code path (settle, capture, hourly reconcile) ever updates it, while the
 * real total flows only to internal billing storage. Customers are billed for
 * storage they cannot see.
 *
 * ONE billable run total: produce a single output file with known contents,
 * wait for the run to settle, then poll the public record for a non-zero
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
    "a settled run retaining a captured output surfaces a non-zero retainedStorageBytes on the public session record",
    async () => {
      const body = `
        const prompt =
          "Use your shell tool to write exactly 2048 bytes to /workspace/outputs/blob.bin " +
          "(for example: head -c 2048 /dev/zero > /workspace/outputs/blob.bin). " +
          "Create no other files. Then reply with the single word done.";
        const runResult = await client.run({
          provider: PROVIDER,
          model: MODEL,
          message: prompt,
          includeBuiltinTools: true,
          outputs: { allowedDirs: ["/workspace/outputs"] },
          apiKeys: { [PROVIDER]: PROVIDER_KEY },
          idempotencyKey: "edge-storage-" + Date.now()
        }, { timeoutMs: 6 * 60_000 });

        const session = await client.sessions.open(runResult.runId);
        const outs = await session.outputs();
        const listed = await outs.list();
        const blob = listed.find((o) => (o.filename || "").endsWith("blob.bin")) || null;

        // Poll the PUBLIC record for a non-zero retained byte total. Settle
        // runs right after the turn finishes; 3 minutes is generous.
        let retained = 0;
        let polls = 0;
        const deadline = Date.now() + 3 * 60_000;
        while (Date.now() < deadline) {
          const rec = (await client.sessions.open(runResult.runId)).record;
          polls++;
          const v = rec.retainedStorageBytes;
          if (typeof v === "number" && v > 0) { retained = v; break; }
          await new Promise((r) => setTimeout(r, 10_000));
        }

        process.stdout.write(JSON.stringify({
          runId: runResult.runId,
          ok: runResult.ok,
          status: runResult.status,
          outputCount: listed.length,
          blobBytes: blob ? blob.sizeBytes ?? null : null,
          retainedStorageBytes: retained,
          polls
        }));
        process.exit(0);
      `;
      const out = (await runChild(install, "edge-storage-A.mjs", body)) as {
        runId: string;
        ok: boolean;
        status: string;
        outputCount: number;
        blobBytes: number | null;
        retainedStorageBytes: number;
        polls: number;
      };

      // The run itself must have produced and retained the output.
      expect(out.ok, `run ${out.runId} did not complete ok (status=${out.status})`).toBe(true);
      expect(
        (out.blobBytes ?? 0) > 0,
        `blob.bin missing or empty in captured outputs (count=${out.outputCount}, bytes=${out.blobBytes})`
      ).toBe(true);

      // DEFECT PROBE: retained-output storage is billed hourly from the
      // settle-recorded byte total, but the public record's
      // retainedStorageBytes is written once as 0 at create and never
      // updated by any later code path — customers pay for retained bytes
      // they cannot see on any public surface. This assertion states the
      // contractually sensible behavior (field reflects retained bytes once
      // the run settles); it fails today and goes green when the plane
      // writes the settle-time outputsSummary total onto the record.
      expect(
        out.retainedStorageBytes,
        `run ${out.runId} retains ${out.blobBytes ?? "?"} bytes of captured output (settled, ` +
          `polled ${out.polls}x over 3 min) but the public session record still reports ` +
          `retainedStorageBytes=0 — retained storage is billed yet invisible to the customer`
      ).toBeGreaterThan(0);
    },
    12 * 60_000
  );
});
