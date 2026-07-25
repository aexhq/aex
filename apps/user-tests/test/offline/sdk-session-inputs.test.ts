import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

const SCRIPT = String.raw`
import { deepStrictEqual, match, ok, strictEqual } from "node:assert/strict";
import { Aex, McpServer, Secret } from "@aexhq/sdk";

const calls = [];
const fetch = async (input, init = {}) => {
  const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
  const body = typeof init.body === "string" ? JSON.parse(init.body) : undefined;
  calls.push({ url, method: init.method ?? "GET", body, headers: init.headers });
  if (url.endsWith("/api/sessions")) {
    return Response.json({ session: { id: "session-1", status: "idle", acceptsMessages: true } }, { status: 201 });
  }
  return Response.json({ error: "unexpected" }, { status: 404 });
};

const hash = "sha256:" + "a".repeat(64);
const assetId = "asset_" + "a".repeat(64);
const base = { resourceId: "wres_" + "1".repeat(32), version: 2, assetId, contentHash: hash };
const file = { ...base, kind: "file", name: "input", mountPath: "/workspace/input" };
const skill = { ...base, resourceId: "wres_" + "2".repeat(32), kind: "skill", name: "report", description: "Report skill." };
const tool = {
  ...base,
  resourceId: "wres_" + "3".repeat(32),
  kind: "tool",
  name: "lookup",
  description: "Lookup tool.",
  input_schema: { type: "object", properties: {} },
  entry: "index.js"
};
const instructions = { ...base, resourceId: "wres_" + "4".repeat(32), kind: "instruction", name: "rules" };

const client = new Aex({ apiKey: "aex_test", baseUrl: "https://api.example", fetch });
await client.sessions.create({

  model: "anthropic/claude-haiku-4-5",
  system: "Be precise.",
  assets: { files: [file], skills: [skill], tools: [tool], instructions: [instructions] },
  builtinTools: ["grep", "bash"],
  mcpServers: [McpServer.remote({ name: "docs", url: "https://mcp.example/sse", headers: { Authorization: "Bearer secret" } })],
  fileCapture: { allowedDirs: ["/workspace/output"], maxFiles: 20 },
  outputMode: "buffered",
  responseFormat: { kind: "text" },
  metadata: { requestId: "req-1" },
  environment: {
    variables: { MODE: "test" },
    secrets: { SERVICE_TOKEN: Secret.value("secret-value") },
    packages: [{ name: "pip:pandas", version: "2.2.0" }]
  },
  runtime: { size: "2cpu-8gb" },
  overrides: { idleTtl: "5m", timeout: "15m", maxSpendUsd: 2, maxTurns: 8 },
  webhook: { url: "https://hooks.example/aex" },
  idempotencyKey: "stable-create"
});

strictEqual(calls.length, 1);
const request = calls[0].body;
deepStrictEqual(request.submission.assets, {
  files: [file],
  skills: [skill],
  tools: [tool],
  instructions: [instructions]
});
deepStrictEqual(request.submission.builtinTools, ["bash", "grep"]);
ok(!("files" in request.submission));
ok(!("skills" in request.submission));
ok(!("tools" in request.submission));
deepStrictEqual(request.submission.environment.envVars, { MODE: "test" });
// Managed keys: the wire carries NO customer provider key, under any name.
ok(!("apiKeys" in request.secrets), "secrets.apiKeys must not be on the wire");
ok(!("apiKey" in request.secrets), "secrets.apiKey must not be on the wire");
ok(!("provider" in request), "the serving provider is derived from the model slug");
deepStrictEqual(request.submission.secretEnv, { SERVICE_TOKEN: { ephemeral: true } });
strictEqual(request.secrets.envSecrets.SERVICE_TOKEN, "secret-value");
strictEqual(request.secrets.mcpServers[0].headers.Authorization, "Bearer secret");
strictEqual(request.runtimeSize, "2cpu-8gb");
deepStrictEqual(request.limits, { maxSpendUsd: 2, maxTurns: 8 });
deepStrictEqual(request.retention, { idleTtl: "5m" });

for (const [field, value] of [
  ["skills", []],
  ["tools", []],
  ["files", []],
  ["includeBuiltinTools", false],
  ["runtimeSize", "2cpu-8gb"],
  ["prompt", "hello"]
]) {
  const before = calls.length;
  let message = "";
  try {
    await client.sessions.create({ model: "anthropic/claude-haiku-4-5", [field]: value });
  } catch (error) {
    message = error.message;
  }
  match(message, new RegExp(field + " is not a supported option"));
  strictEqual(calls.length, before, field + " must fail before network access");
}

process.stdout.write(JSON.stringify({
  assetKinds: Object.keys(request.submission.assets),
  builtinTools: request.submission.builtinTools,
  runtimeSize: request.runtimeSize,
  rejectedLegacyFields: 6
}));
`;

describe("installed SDK session input contract", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex();
  }, 240_000);

  afterAll(() => install?.cleanup());

  it("serializes rich clean-cut options and rejects removed fields", async () => {
    const path = join(install.installDir, "sdk-session-inputs.mjs");
    writeFileSync(path, SCRIPT);
    const child = await runCommand(getBunCommand(), [path], { cwd: install.installDir, timeoutMs: 120_000 });
    if (child.exitCode !== 0) throw new Error(`sdk-session-inputs.mjs exited ${child.exitCode}\n${child.stderr}`);
    expect(JSON.parse(child.stdout)).toEqual({
      assetKinds: ["files", "skills", "tools", "instructions"],
      builtinTools: ["bash", "grep"],
      runtimeSize: "2cpu-8gb",
      rejectedLegacyFields: 6
    });
  });
});
