import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { join } from "node:path";
import { writeFileSync } from "node:fs";
import {
  getBunCommand,
  installAex,
  runCommand,
  type InstallResult
} from "../_fixtures/install.js";

let install: InstallResult;

beforeAll(async () => {
  install = await installAex();
}, 300_000);

afterAll(() => {
  install.cleanup();
});

describe("packed strict v1 artifacts", () => {
  it("installs the SDK and standalone CLI without workspace runtime dependencies", () => {
    expect(install.sdkPackageJson.name).toBe("@aexhq/sdk");
    expect(install.cliPackageJson.name).toBe("@aexhq/cli");
    expect(install.sdkPackageJson.bin).toBeUndefined();
    expect(install.sdkPackageJson.dependencies).toEqual({ zod: "4.4.3" });
    expect(install.cliPackageJson.bin).toEqual({ aex: "./dist/cli.mjs" });
    expect(install.cliPackageJson.dependencies).toBeUndefined();
  });

  it("exercises sessions, operations, registries, files, telemetry, and account errors", async () => {
    const script = join(install.installDir, "strict-v1.mjs");
    writeFileSync(script, STRICT_V1_SCRIPT);
    const result = await runCommand(getBunCommand(), [script], {
      cwd: install.installDir,
      timeoutMs: 60_000
    });

    expect(result.exitCode, result.stderr).toBe(0);
    expect(JSON.parse(result.stdout)).toEqual({
      runStatus: "succeeded",
      persistedBytes: 4,
      liveGeneration: "gen_01kyrrm24kffnsxd9we2qzav00",
      registryStatus: "replaced",
      registryRevision: 2,
      registeredFileBytes: 4,
      streamFrames: ["checkpoint"],
      exportId: "exp_01kyrrm24kftatca1xh1j0g5nx",
      accountError: "account_paused",
      billingError: "account_state_unavailable"
    });
  });

  it("ships the separate resource-oriented CLI and rejects the retired one-shot command", async () => {
    const cli = join(install.cliDir, "dist", "cli.mjs");
    const help = await runCommand(getBunCommand(), [cli, "help"], {
      cwd: install.installDir
    });
    expect(help.exitCode, help.stderr).toBe(0);
    expect(help.stdout).toContain("sessions create|list|get|stop|persist|fork|delete");
    expect(help.stdout).toContain("files live|persisted");
    expect(help.stdout).not.toContain("aex start");

    const removed = await runCommand(getBunCommand(), [cli, "start"], {
      cwd: install.installDir
    });
    expect(removed.exitCode).toBe(2);
    expect(removed.stderr).toContain("removed command: start");
  });
});

const STRICT_V1_SCRIPT = String.raw`
import { strict as assert } from "node:assert";
import * as sdk from "@aexhq/sdk";

const SID = "ses_01kyrrm24jebjtevevjjxab3pp";
const WID = "wsp_01kyrrm24kfbhsv3szhtavs56n";
const MID = "msg_01kyrrm24kfggbq5408tsj3y3k";
const RID = "run_01kyrrm24kftp9xc7w35e53bn0";
const GEN = "gen_01kyrrm24kffnsxd9we2qzav00";
const MSR = "msr_01kyrrm24kewft8vpmd0vkfdew";
const EXP = "exp_01kyrrm24kftatca1xh1j0g5nx";
const NOW = "2026-07-30T10:00:00.000Z";
const HASH = "sha256:" + "0".repeat(64);
const calls = [];

assert.equal(typeof sdk.Aex, "function");
for (const removed of [
  "SDK_VERSION", "AssetRef", "RuntimeKinds", "File", "Skill", "Tool",
  "SessionResult", "StartSessionOptions"
]) {
  assert.equal(Object.hasOwn(sdk, removed), false, removed);
}

const session = {
  id: SID,
  workspaceId: WID,
  status: "idle",
  revision: 1,
  persistRevision: 0,
  createdAt: NOW,
  updatedAt: NOW,
  continuity: {
    state: "cold",
    persistedRevision: 0,
    changedAt: NOW,
    reason: "not_started"
  },
  lineage: {},
  resolvedConfig: {}
};
const grant = {
  url: "https://download.example/file",
  expiresAt: NOW,
  sizeBytes: 4,
  authorizedBytes: 4,
  measurementId: MSR,
  sha256: HASH
};

const json = (value, status = 200) =>
  new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json", "x-request-id": "req_test" }
  });

const fetch = async (input, init = {}) => {
  const url = new URL(input instanceof URL ? input.href : String(input));
  const method = init.method ?? "GET";
  const headers = new Headers(init.headers);
  const body = typeof init.body === "string" ? JSON.parse(init.body) : undefined;
  calls.push({ method, path: url.pathname, headers, body });

  if (method === "POST" && url.pathname === "/api/sessions") return json(session, 201);
  if (method === "POST" && url.pathname === "/api/sessions/" + SID + "/messages") {
    return json({
      message: {
        id: MID,
        sessionId: SID,
        runId: RID,
        role: "user",
        content: [{ type: "text", text: "hello" }],
        createdAt: NOW
      },
      run: {
        id: RID,
        sessionId: SID,
        messageId: MID,
        status: "queued",
        maxSpendCents: 100,
        queuedAt: NOW
      }
    }, 201);
  }
  if (method === "GET" && url.pathname === "/api/sessions/" + SID + "/runs/" + RID) {
    return json({
      id: RID,
      sessionId: SID,
      messageId: MID,
      status: "succeeded",
      maxSpendCents: 100,
      queuedAt: NOW,
      terminalAt: NOW,
      outputMessageIds: []
    });
  }
  if (method === "PUT" && url.pathname === "/api/workspace/instructions/rules") {
    return json({
      status: "replaced",
      resource: {
        kind: "instruction",
        name: "rules",
        revision: 2,
        state: "current",
        sha256: HASH,
        sizeBytes: 8,
        createdAt: NOW,
        updatedAt: NOW,
        value: body
      }
    });
  }
  if (method === "PUT" && url.pathname === "/api/workspace/files/repository-context") {
    return json({
      status: "created",
      resource: {
        kind: "file",
        name: "repository-context",
        revision: 1,
        state: "current",
        sha256: HASH,
        sizeBytes: 4,
        createdAt: NOW,
        updatedAt: NOW,
        value: {
          mountPath: "context.txt",
          content: { sha256: HASH, sizeBytes: 4 },
          mediaType: "text/plain",
          mode: "0644"
        }
      }
    });
  }
  if (method === "POST" && url.pathname === "/api/workspace/files/repository-context/downloads") {
    return json(grant, 201);
  }
  if (method === "POST" && url.pathname.endsWith("/files/persisted/downloads")) {
    return json(grant, 201);
  }
  if (method === "POST" && url.pathname.endsWith("/files/live/downloads")) {
    return json({
      ...grant,
      workspaceAccess: { generationId: GEN, resumed: false }
    }, 201);
  }
  if (method === "POST" && url.pathname === "/api/telemetry/query") {
    return json({
      items: [],
      coverage: {
        snapshot: { cursor: "cur_1", time: NOW },
        accepted: { cursor: "cur_1", time: NOW },
        indexed: { cursor: "cur_1", time: NOW },
        earliestReplay: { cursor: "cur_1", time: NOW },
        caughtUp: true,
        complete: true,
        missingIntervals: [],
        unboundedGaps: 0
      }
    });
  }
  if (method === "POST" && url.pathname === "/api/telemetry/stream") {
    return new Response('{"type":"checkpoint","cursor":"cur_1"}\n', {
      headers: { "content-type": "application/x-ndjson" }
    });
  }
  if (method === "POST" && url.pathname === "/api/telemetry/exports") {
    const id = headers.get("aex-operation-id");
    return json({
      id,
      workspaceId: WID,
      kind: "telemetry_export",
      status: "queued",
      cancelable: true,
      createdAt: NOW,
      updatedAt: NOW
    }, 202);
  }
  if (method === "GET" && url.pathname.startsWith("/api/operations/")) {
    const id = url.pathname.split("/").at(-1);
    return json({
      id,
      workspaceId: WID,
      kind: "telemetry_export",
      status: "succeeded",
      cancelable: false,
      createdAt: NOW,
      updatedAt: NOW,
      terminalAt: NOW,
      result: {
        exportId: EXP,
        format: "ndjson",
        manifestHash: HASH,
        expiresAt: NOW
      }
    });
  }
  if (method === "GET" && url.pathname === "/api/account") {
    return json({ error: "account_paused", message: "top up required", requestId: "req_account" }, 402);
  }
  if (method === "GET" && url.pathname === "/api/billing/balance") {
    return json({
      error: "account_state_unavailable",
      message: "billing state unavailable",
      requestId: "req_billing"
    }, 503);
  }
  throw new Error("unexpected request: " + method + " " + url.pathname);
};

const aex = new sdk.Aex({
  apiKey: "test-key",
  baseUrl: "https://regional.example",
  fetch,
  retry: false
});
assert.equal(Object.hasOwn(aex, "start"), false);
assert.equal(Object.hasOwn(aex, "runtimeCapabilities"), false);

const opened = await aex.sessions.create({ model: "openai/gpt-5" });
assert.equal(Object.hasOwn(opened, "checkpoint"), false);
assert.equal(Object.hasOwn(opened, "webhooks"), false);
assert.equal(Object.hasOwn(opened, "children"), false);
const accepted = await opened.messages.send("hello");
const run = await accepted.run.result({ pollIntervalMs: 0 });

const registered = await aex.workspace.instructions.set(
  "rules",
  { text: "be exact" },
  { ifRevision: 1, idempotencyKey: "overwrite-rules" }
);
assert.equal(registered.status, "replaced");
assert.equal(registered.resource.revision, 2);
assert.equal(Object.hasOwn(aex.workspace.instructions, "versions"), false);
assert.equal(Object.hasOwn(aex.workspace.instructions, "history"), false);

const registeredFile = await aex.workspace.files.set(
  "repository-context",
  {
    mountPath: "context.txt",
    content: {
      type: "inline",
      encoding: "utf8",
      data: "test",
      sha256: HASH
    },
    mediaType: "text/plain",
    mode: "0644"
  },
  { idempotencyKey: "create-repository-context" }
);
assert.deepEqual(registeredFile.resource.value.content, {
  sha256: HASH,
  sizeBytes: 4
});
assert.equal(Object.hasOwn(registeredFile.resource.value.content, "data"), false);
assert.equal(Object.hasOwn(registeredFile.resource.value.content, "uploadId"), false);
await aex.workspace.files.download("repository-context");

const persisted = await opened.files.persisted.download({ path: "report.txt" });
const live = await opened.files.live.download({
  path: "report.txt",
  wake: "retained",
  consistency: "coherent",
  ifGenerationId: GEN
});
await aex.telemetry.query({ limit: 1 });
const streamFrames = [];
for await (const frame of aex.telemetry.stream({})) streamFrames.push(frame.type);
const exportOperation = await aex.telemetry.export({
  query: {},
  format: "ndjson",
  completeness: "require"
});
const exportResult = await exportOperation.result({ pollIntervalMs: 0 });

let accountError;
try {
  await aex.account.get();
} catch (error) {
  assert.ok(error instanceof sdk.AexApiError);
  assert.equal(error.status, 402);
  accountError = error.apiCode;
}
let billingError;
try {
  await aex.billing.balance.get();
} catch (error) {
  assert.ok(error instanceof sdk.AexApiError);
  assert.equal(error.status, 503);
  billingError = error.apiCode;
}

assert.equal(calls.some((call) => call.path.includes("/versions")), false);
process.stdout.write(JSON.stringify({
  runStatus: run.status,
  persistedBytes: persisted.authorizedBytes,
  liveGeneration: live.workspaceAccess.generationId,
  registryStatus: registered.status,
  registryRevision: registered.resource.revision,
  registeredFileBytes: registeredFile.resource.value.content.sizeBytes,
  streamFrames,
  exportId: exportResult.exportId,
  accountError,
  billingError
}));
`;
