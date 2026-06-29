/**
 * Blackbox SDK submit-input coverage through a clean installed package.
 *
 * These cases intentionally run in child processes whose cwd is the
 * user-test install tempdir. That keeps the assertions at the user boundary:
 * `import "@aexhq/sdk"` resolves from the packed or published artifact, while
 * a fake fetch captures the exact SDK wire request without dispatching a live
 * run.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

const CHILD_HARNESS = String.raw`
import { deepStrictEqual, match, ok, strictEqual } from "node:assert/strict";

function headersToObject(headers) {
  const out = {};
  if (!headers) return out;
  if (headers instanceof Headers) {
    for (const [key, value] of headers.entries()) out[key.toLowerCase()] = value;
    return out;
  }
  if (Array.isArray(headers)) {
    for (const [key, value] of headers) out[String(key).toLowerCase()] = String(value);
    return out;
  }
  for (const [key, value] of Object.entries(headers)) out[String(key).toLowerCase()] = String(value);
  return out;
}

async function decodeBody(body) {
  if (body === undefined || body === null) return undefined;
  if (typeof body === "string") {
    try {
      return JSON.parse(body);
    } catch {
      return body;
    }
  }
  if (body instanceof Uint8Array) {
    return { kind: "Uint8Array", byteLength: body.byteLength };
  }
  if (body instanceof ArrayBuffer) {
    return { kind: "ArrayBuffer", byteLength: body.byteLength };
  }
  if (body instanceof Blob) {
    return { kind: "Blob", byteLength: body.size };
  }
  try {
    const text = await new Response(body).text();
    try {
      return JSON.parse(text);
    } catch {
      return text;
    }
  } catch {
    return String(body);
  }
}

function makeFetch() {
  const calls = [];
  let runCounter = 0;
  const fetchFake = async (input, init = {}) => {
    const url =
      typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const method = String(init.method ?? "GET").toUpperCase();
    const headers = headersToObject(init.headers);
    const body = await decodeBody(init.body);
    calls.push({ url, method, headers, body });

    if (url.endsWith("/assets/presign")) {
      const hash = body && typeof body.hash === "string" ? body.hash : "sha256:" + "a".repeat(64);
      const hex = hash.startsWith("sha256:") ? hash.slice("sha256:".length) : hash;
      return new Response(JSON.stringify({
        ok: true,
        exists: false,
        assetId: "asset_" + hex,
        contentHash: hash,
        uploadUrl: "https://object-storage.example.test/assets/" + hex + "?sig=test",
        requiredHeaders: { "x-amz-checksum-sha256": "Y2hlY2tzdW0=" },
        expiresInSeconds: 300
      }), { status: 201, headers: { "content-type": "application/json" } });
    }

    if (url.includes("object-storage.example.test")) {
      return new Response("", { status: 200 });
    }

    if (url.endsWith("/assets/finalize")) {
      const hash = body && typeof body.hash === "string" ? body.hash : "sha256:" + "a".repeat(64);
      const hex = hash.startsWith("sha256:") ? hash.slice("sha256:".length) : hash;
      return new Response(JSON.stringify({
        ok: true,
        exists: false,
        assetId: "asset_" + hex,
        contentHash: hash,
        sizeBytes: body && typeof body.sizeBytes === "number" ? body.sizeBytes : 0
      }), { status: 200, headers: { "content-type": "application/json" } });
    }

    if (url.endsWith("/api/runs")) {
      runCounter += 1;
      return new Response(JSON.stringify({
        id: "run_user_input_" + runCounter,
        workspaceId: "ws_user_inputs",
        status: "queued",
        createdAt: new Date(0).toISOString()
      }), { status: 202, headers: { "content-type": "application/json" } });
    }

    if (url.endsWith("/api/secrets")) {
      return new Response(JSON.stringify({
        name: body && typeof body.name === "string" ? body.name : "secret",
        version: 1,
        createdAt: new Date(0).toISOString(),
        updatedAt: new Date(0).toISOString()
      }), { status: 201, headers: { "content-type": "application/json" } });
    }

    return new Response(JSON.stringify({ ok: true }), {
      status: 200,
      headers: { "content-type": "application/json" }
    });
  };
  return { calls, fetch: fetchFake };
}

function runBodies(calls) {
  return calls.filter((call) => call.url.endsWith("/api/runs")).map((call) => call.body);
}

function onlyRunBody(calls) {
  const bodies = runBodies(calls);
  strictEqual(bodies.length, 1, "expected exactly one submit call");
  return bodies[0];
}

function presignCalls(calls) {
  return calls.filter((call) => call.url.endsWith("/assets/presign"));
}

function finalizeCalls(calls) {
  return calls.filter((call) => call.url.endsWith("/assets/finalize"));
}

function storagePuts(calls) {
  return calls.filter((call) => call.url.includes("object-storage.example.test") && call.method === "PUT");
}

function resetCalls(calls) {
  calls.length = 0;
}

async function expectReject(label, fn, pattern) {
  try {
    await fn();
  } catch (err) {
    const message = err && err.message ? err.message : String(err);
    if (pattern) match(message, pattern, label + " message");
    return message;
  }
  throw new Error(label + " unexpectedly resolved");
}

function assetIdFromHash(hash) {
  const hex = hash.startsWith("sha256:") ? hash.slice("sha256:".length) : hash;
  return "asset_" + hex;
}
`;

describe("SDK submit user inputs (installed package)", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex();
  }, 240_000);

  afterAll(() => {
    install?.cleanup();
  });

  async function runChild(
    script: string,
    fileName: string,
    timeoutMs = 120_000
  ): Promise<Record<string, unknown>> {
    const scriptPath = join(install.installDir, fileName);
    writeFileSync(scriptPath, script);
    const child = await runCommand(getBunCommand(), [scriptPath], {
      cwd: install.installDir,
      timeoutMs
    });
    if (child.exitCode !== 0) {
      throw new Error(
        `${fileName} exited ${child.exitCode}\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
      );
    }
    return JSON.parse(child.stdout.trim()) as Record<string, unknown>;
  }

  it("serializes every major submit input into the public wire request", async () => {
    const script = CHILD_HARNESS + String.raw`
const {
  AgentExecutor,
  AgentsMd,
  BuiltinTools,
  File,
  McpServer,
  ProxyEndpoint,
  RuntimeSizes,
  Secret,
  Skill,
  Tool
} = await import("@aexhq/sdk");

const { calls, fetch } = makeFetch();
const client = new AgentExecutor({
  apiToken: "aex_user_inputs_token",
  baseUrl: "https://example.invalid",
  fetch
});

const skill = await Skill.fromFiles({
  name: "alpha-skill",
  files: { "SKILL.md": "# Alpha\nUse the alpha behavior.\n" }
});
const skillHash = skill.ref.contentHash;
const catalogHash = "b".repeat(64);
const catalogSkill = Skill.fromCatalog({ name: "catalog-skill", hash: "sha256:" + catalogHash });
const tool = await Tool.fromFiles({
  name: "lookup_tool",
  description: "Looks up one test value.",
  inputSchema: { type: "object", properties: { q: { type: "string" } }, required: ["q"] },
  entry: "index.js",
  files: { "index.js": "export default async function () { return { content: [] }; }\n" }
});
const toolHash = tool.ref.contentHash;
const agentsMd = await AgentsMd.fromContent("# Workspace rules\nKeep answers short.\n", { name: "rules" });
const agentsHash = agentsMd.ref.contentHash;
const dataFile = await File.fromBytes({
  name: "dataset.csv",
  bytes: new TextEncoder().encode("id,value\n1,alpha\n"),
  mountPath: "/workspace/input"
});
const fileHash = dataFile.ref.contentHash;

const runId = await client.submit({
  model: "claude-haiku-4-5",
  system: "System instructions.",
  prompt: ["first user turn", "second user turn"],
  skills: [skill, catalogSkill],
  tools: [BuiltinTools.web_fetch, BuiltinTools.web_fetch, tool],
  agentsMd: [agentsMd],
  files: [dataFile],
  mcpServers: [
    McpServer.remote({
      name: "docs",
      url: "https://mcp.example.test/sse",
      transport: "sse",
      headers: { Authorization: "Bearer mcp-secret" }
    }),
    McpServer.fromId("mcp_abcdefgh12345678")
  ],
  secretEnv: {
    EPHEMERAL_TOKEN: Secret.value("ephemeral-secret-value"),
    WORKSPACE_TOKEN: Secret.ref("workspace-secret")
  },
  environment: {
    networking: { mode: "limited", allowedHosts: ["api.example.test"] },
    envVars: { PLAIN_ENV: "visible" },
    packages: [{ name: "apt:jq" }, { name: "pip:pandas" }]
  },
  metadata: { suite: "sdk-submit-user-inputs", nested: { count: 2 } },
  runtimeSize: RuntimeSizes.SHARED_2X_8GB,
  timeout: "90m",
  postHook: { command: "bun test", timeout: "2m", maxTurns: 2, maxChars: 4096 },
  proxyEndpoints: [
    ProxyEndpoint.bearer({
      name: "catalog",
      baseUrl: "https://api.example.test",
      token: "proxy-bearer-secret",
      allowMethods: ["GET", "POST"],
      allowPathPrefixes: ["/v1"],
      allowHeaders: ["accept"],
      responseMode: "headers_only",
      maxRequestBytes: 12345,
      maxResponseBytes: 67890,
      timeoutMs: 5000,
      retry: { maxAttempts: 2, backoffMs: 100 }
    }),
    ProxyEndpoint.none({
      name: "public",
      baseUrl: "https://public.example.test",
      allowMethods: ["GET"],
      allowPathPrefixes: ["/"]
    })
  ],
  outputs: {
    allowedDirs: ["/workspace/out", ""],
    deniedDirs: ["", "/workspace/out/tmp"],
    captureTimeoutMs: 120000,
    maxFileBytes: 1000000,
    maxTotalBytes: 2000000,
    maxFiles: 25
  },
  includeBuiltinTools: false,
  outputMode: "stream",
  secrets: { apiKeys: { anthropic: "sk-ant-user-inputs" } },
  idempotencyKey: "idem-user-inputs",
  parentRunId: "run_parent_123",
  webhook: { url: "https://hooks.example.test/aex" },
  limits: { maxConcurrentChildRuns: 8, maxSubagentDepth: 3 }
});

strictEqual(runId, "run_user_input_1");
strictEqual(presignCalls(calls).length, 4);
strictEqual(storagePuts(calls).length, 4);
strictEqual(finalizeCalls(calls).length, 4);

const body = onlyRunBody(calls);
strictEqual(body.idempotencyKey, "idem-user-inputs");
strictEqual(body.provider, "anthropic");
strictEqual(body.runtimeSize, "shared-2x-8gb");
strictEqual(body.timeout, "90m");
strictEqual(body.parentRunId, "run_parent_123");
deepStrictEqual(body.webhook, { url: "https://hooks.example.test/aex" });
deepStrictEqual(body.limits, { maxConcurrentChildRuns: 8, maxSubagentDepth: 3 });
deepStrictEqual(body.postHook, { command: "bun test", timeout: "2m", maxTurns: 2, maxChars: 4096 });

const submission = body.submission;
strictEqual(submission.model, "claude-haiku-4-5");
strictEqual(submission.system, "System instructions.");
deepStrictEqual(submission.prompt, ["first user turn", "second user turn"]);
deepStrictEqual(submission.metadata, { suite: "sdk-submit-user-inputs", nested: { count: 2 } });
deepStrictEqual(submission.environment, {
  networking: { mode: "limited", allowedHosts: ["api.example.test"] },
  envVars: { PLAIN_ENV: "visible" },
  packages: [{ name: "apt:jq" }, { name: "pip:pandas" }]
});
strictEqual(submission.includeBuiltinTools, false);
strictEqual(submission.outputMode, "stream");
deepStrictEqual(submission.outputs, {
  allowedDirs: ["/workspace/out"],
  deniedDirs: ["/workspace/out/tmp"],
  captureTimeoutMs: 120000,
  maxFileBytes: 1000000,
  maxTotalBytes: 2000000,
  maxFiles: 25
});
deepStrictEqual(submission.skills, [
  { kind: "asset", assetId: assetIdFromHash(skillHash), name: "alpha-skill" },
  { kind: "asset", assetId: "asset_" + catalogHash, name: "catalog-skill" }
]);
strictEqual(submission.tools[0], "web_fetch");
strictEqual(submission.tools.length, 2, "duplicate builtin names should be deduped");
deepStrictEqual(submission.tools[1], {
  kind: "asset",
  assetId: assetIdFromHash(toolHash),
  name: "lookup_tool",
  description: "Looks up one test value.",
  input_schema: { type: "object", properties: { q: { type: "string" } }, required: ["q"] },
  entry: "index.js"
});
deepStrictEqual(submission.agentsMd, [
  { kind: "asset", assetId: assetIdFromHash(agentsHash), name: "rules" }
]);
deepStrictEqual(submission.files, [
  { kind: "asset", assetId: assetIdFromHash(fileHash), name: "dataset", mountPath: "/workspace/input" }
]);
deepStrictEqual(submission.mcpServers, [
  { name: "docs", transport: "sse", url: "https://mcp.example.test/sse" },
  { kind: "workspace", id: "mcp_abcdefgh12345678" }
]);
deepStrictEqual(submission.secretEnv, {
  EPHEMERAL_TOKEN: { ephemeral: true },
  WORKSPACE_TOKEN: { ref: "workspace-secret" }
});
ok(!JSON.stringify(submission).includes("ephemeral-secret-value"));

deepStrictEqual(body.proxyEndpoints.map((endpoint) => endpoint.name), ["catalog", "public"]);
deepStrictEqual(body.secrets.apiKeys, { anthropic: "sk-ant-user-inputs" });
deepStrictEqual(body.secrets.envSecrets, { EPHEMERAL_TOKEN: "ephemeral-secret-value" });
deepStrictEqual(body.secrets.mcpServers, [
  { name: "docs", url: "https://mcp.example.test/sse", headers: { Authorization: "Bearer mcp-secret" } }
]);
deepStrictEqual(body.secrets.proxyEndpointAuth, [
  { name: "catalog", value: { type: "bearer", token: "proxy-bearer-secret" } }
]);

console.log(JSON.stringify({
  ok: true,
  submitCalls: runBodies(calls).length,
  assetUploads: presignCalls(calls).length,
  skillNames: submission.skills.map((entry) => entry.name),
  toolEntries: submission.tools.length
}));
`;
    const result = await runChild(script, "sdk-submit-user-inputs-full.mjs");
    expect(result).toMatchObject({
      ok: true,
      submitCalls: 1,
      assetUploads: 4,
      skillNames: ["alpha-skill", "catalog-skill"],
      toolEntries: 2
    });
  });

  it("covers skill absence, fake skills, catalog skills, ordering, reuse, and many-skill stress", async () => {
    const script = CHILD_HARNESS + String.raw`
const { AgentExecutor, Skill } = await import("@aexhq/sdk");

function makeClient() {
  const harness = makeFetch();
  const client = new AgentExecutor({
    apiToken: "aex_skill_inputs_token",
    baseUrl: "https://example.invalid",
    fetch: harness.fetch
  });
  return { ...harness, client };
}

const none = makeClient();
await none.client.submit({
  model: "claude-haiku-4-5",
  prompt: "no skills",
  secrets: { apiKeys: { anthropic: "sk-ant" } }
});
deepStrictEqual(onlyRunBody(none.calls).submission.skills, []);
strictEqual(presignCalls(none.calls).length, 0);

const fake = makeClient();
await expectReject("fake skill object", () => fake.client.submit({
  model: "claude-haiku-4-5",
  prompt: "fake skill",
  skills: [{ kind: "workspace", id: "skill_does_not_exist" }],
  secrets: { apiKeys: { anthropic: "sk-ant" } }
}), /skills\[0\] must be a Skill instance/);
strictEqual(fake.calls.length, 0);

const catalog = makeClient();
const catalogHash = "c".repeat(64);
await catalog.client.submit({
  model: "claude-haiku-4-5",
  prompt: "catalog skill",
  skills: [Skill.fromCatalog({ name: "existing-catalog-skill", hash: "sha256:" + catalogHash })],
  secrets: { apiKeys: { anthropic: "sk-ant" } }
});
deepStrictEqual(onlyRunBody(catalog.calls).submission.skills, [
  { kind: "asset", assetId: "asset_" + catalogHash, name: "existing-catalog-skill" }
]);
strictEqual(presignCalls(catalog.calls).length, 0);

const order = makeClient();
const orderedSkills = [];
for (let i = 0; i < 6; i += 1) {
  orderedSkills.push(await Skill.fromFiles({
    name: "ordered-skill-" + i,
    files: { "SKILL.md": "# Skill " + i + "\nReply with order " + i + ".\n" }
  }));
}
await order.client.submit({
  model: "claude-haiku-4-5",
  prompt: "ordered skills",
  skills: orderedSkills,
  secrets: { apiKeys: { anthropic: "sk-ant" } }
});
deepStrictEqual(
  onlyRunBody(order.calls).submission.skills.map((entry) => entry.name),
  orderedSkills.map((skill) => skill.ref.name)
);
strictEqual(presignCalls(order.calls).length, 6);

const repeated = makeClient();
const reusable = await Skill.fromFiles({
  name: "reusable-skill",
  files: { "SKILL.md": "# Reusable\nApply this every time.\n" }
});
const reusableAssetId = assetIdFromHash(reusable.ref.contentHash);
await repeated.client.submit({
  model: "claude-haiku-4-5",
  prompt: "same skill repeated in one run",
  skills: [reusable, reusable, reusable],
  secrets: { apiKeys: { anthropic: "sk-ant" } }
});
deepStrictEqual(onlyRunBody(repeated.calls).submission.skills, [
  { kind: "asset", assetId: reusableAssetId, name: "reusable-skill" },
  { kind: "asset", assetId: reusableAssetId, name: "reusable-skill" },
  { kind: "asset", assetId: reusableAssetId, name: "reusable-skill" }
]);
strictEqual(presignCalls(repeated.calls).length, 1);

resetCalls(repeated.calls);
await repeated.client.submit({
  model: "claude-haiku-4-5",
  prompt: "same skill reused in a later run",
  skills: [reusable],
  secrets: { apiKeys: { anthropic: "sk-ant" } }
});
deepStrictEqual(onlyRunBody(repeated.calls).submission.skills, [
  { kind: "asset", assetId: reusableAssetId, name: "reusable-skill" }
]);
strictEqual(presignCalls(repeated.calls).length, 0);

const stress = makeClient();
const many = [];
for (let i = 0; i < 64; i += 1) {
  const name = "stress-skill-" + String(i).padStart(2, "0");
  many.push(await Skill.fromFiles({
    name,
    files: { "SKILL.md": "# " + name + "\nReturn token " + i + ".\n" }
  }));
}
await stress.client.submit({
  model: "claude-haiku-4-5",
  prompt: "many skills",
  skills: many,
  secrets: { apiKeys: { anthropic: "sk-ant" } }
});
const stressSkills = onlyRunBody(stress.calls).submission.skills;
strictEqual(stressSkills.length, 64);
strictEqual(stressSkills[0].name, "stress-skill-00");
strictEqual(stressSkills[63].name, "stress-skill-63");
deepStrictEqual(stressSkills.map((entry) => entry.name), many.map((skill) => skill.ref.name));
strictEqual(presignCalls(stress.calls).length, 64);
strictEqual(storagePuts(stress.calls).length, 64);
strictEqual(finalizeCalls(stress.calls).length, 64);

console.log(JSON.stringify({
  ok: true,
  orderedCount: orderedSkills.length,
  stressCount: stressSkills.length,
  repeatedUploadCount: presignCalls(repeated.calls).length,
  fakeCalls: fake.calls.length
}));
`;
    const result = await runChild(script, "sdk-submit-user-inputs-skills.mjs", 180_000);
    expect(result).toMatchObject({
      ok: true,
      orderedCount: 6,
      stressCount: 64,
      repeatedUploadCount: 0,
      fakeCalls: 0
    });
  });

  it("rejects invalid submit input and primitive-builder edge cases before posting", async () => {
    const script = CHILD_HARNESS + String.raw`
const {
  AgentExecutor,
  AgentsMd,
  File,
  McpServer,
  ProxyEndpoint,
  Secret,
  Skill,
  Tool
} = await import("@aexhq/sdk");

const { calls, fetch } = makeFetch();
const client = new AgentExecutor({
  apiToken: "aex_invalid_inputs_token",
  baseUrl: "https://example.invalid",
  fetch
});
const validBase = {
  model: "claude-haiku-4-5",
  prompt: "valid prompt",
  secrets: { apiKeys: { anthropic: "sk-ant" } }
};

const submitCases = [
  ["missing options", () => client.submit(undefined), /options is required/],
  ["empty prompt string", () => client.submit({ ...validBase, prompt: "" }), /prompt must be a non-empty string/],
  ["empty prompt array", () => client.submit({ ...validBase, prompt: [] }), /prompt must be a non-empty string or string array/],
  ["empty prompt segment", () => client.submit({ ...validBase, prompt: ["ok", ""] }), /prompt segments must be non-empty strings/],
  ["missing provider key", () => client.submit({ model: "claude-haiku-4-5", prompt: "p" }), /provider API key is required/],
  ["provider mismatch", () => client.submit({
    model: "gpt-4.1",
    provider: "anthropic",
    prompt: "p",
    secrets: { apiKeys: { anthropic: "sk-ant" } }
  }), /provider "anthropic" is not available/],
  ["removed runtime", () => client.submit({ ...validBase, runtime: "managed" }), /runtime is not a supported option/],
  ["removed region", () => client.submit({ ...validBase, region: "us-west-2" }), /region is not a supported option/],
  ["removed credentialMode", () => client.submit({ ...validBase, credentialMode: "byok" }), /credentialMode is not a supported option/],
  ["removed apiKey", () => client.submit({ ...validBase, apiKey: "sk-ant" }), /apiKey is not a supported option/],
  ["removed credentials", () => client.submit({ ...validBase, credentials: { anthropic: "sk-ant" } }), /credentials is not a supported option/],
  ["removed secrets.apiKey", () => client.submit({
    model: "claude-haiku-4-5",
    prompt: "p",
    secrets: { apiKey: "sk-ant" }
  }), /secrets\.apiKey is not supported/],
  ["bad skill entry", () => client.submit({ ...validBase, skills: [{}] }), /skills\[0\] must be a Skill instance/],
  ["bad tool entry", () => client.submit({ ...validBase, tools: ["definitely_not_builtin"] }), /not a builtin tool name/],
  ["bad agentsMd entry", () => client.submit({ ...validBase, agentsMd: [{}] }), /agentsMd\[0\] must be an AgentsMd instance/],
  ["bad file entry", () => client.submit({ ...validBase, files: [{}] }), /files\[0\] must be a File instance/],
  ["bad mcp entry", () => client.submit({ ...validBase, mcpServers: [{}] }), /mcpServers\[0\] must be an McpServer instance/],
  ["bad proxy entry", () => client.submit({ ...validBase, proxyEndpoints: [{}] }), /proxyEndpoints\[0\] must be a ProxyEndpoint/],
  ["bad secret env name", () => client.submit({ ...validBase, secretEnv: { "bad-name": Secret.value("secret") } }), /env var name/],
  ["bad secret env value", () => client.submit({ ...validBase, secretEnv: { VALID_NAME: "secret" } }), /must be a Secret/],
  ["bad limits value", () => client.submit({ ...validBase, limits: { maxConcurrentChildRuns: 0 } }), /must be a positive/],
  ["bad limits field", () => client.submit({ ...validBase, limits: { maxDepth: 3 } }), /not an allowed field/]
];

const messages = [];
for (const [label, fn, pattern] of submitCases) {
  messages.push(await expectReject(label, fn, pattern));
}
strictEqual(calls.length, 0, "invalid submit inputs must not make HTTP calls");

const builderCases = [
  ["skill missing args", () => Skill.fromFiles(undefined), /args is required/],
  ["skill bad name", () => Skill.fromFiles({ name: "Bad Name", files: { "SKILL.md": "# x" } }), /name must match/],
  ["skill missing skill md", () => Skill.fromFiles({ name: "valid-name", files: { "README.md": "x" } }), /SKILL\.md/],
  ["catalog missing hash", () => Skill.fromCatalog({ name: "valid-name", hash: null }), /sha256|ready/],
  ["agents empty content", () => AgentsMd.fromContent("", { name: "rules" }), /content must be a non-empty string/],
  ["agents bad name", () => AgentsMd.fromContent("# x", { name: "Bad" }), /name must match/],
  ["file bad name", () => File.fromBytes({ name: "../secret.txt", bytes: new Uint8Array([1]) }), /valid filename/],
  ["file empty bytes", () => File.fromBytes({ name: "empty.txt", bytes: new Uint8Array() }), /non-empty Uint8Array/],
  ["file bad mount", () => File.fromBytes({ name: "ok.txt", bytes: new Uint8Array([1]), mountPath: "../bad" }), /absolute path/],
  ["tool bad name", () => Tool.fromFiles({
    name: "Bad Tool",
    description: "d",
    inputSchema: { type: "object", properties: {} },
    entry: "index.js",
    files: { "index.js": "" }
  }), /name must match/],
  ["tool missing schema", () => Tool.fromFiles({
    name: "good_tool",
    description: "d",
    entry: "index.js",
    files: { "index.js": "" }
  }), /inputSchema/],
  ["tool missing description", () => Tool.fromFiles({
    name: "good_tool",
    description: "",
    inputSchema: { type: "object", properties: {} },
    entry: "index.js",
    files: { "index.js": "" }
  }), /description/],
  ["mcp bad id", () => McpServer.fromId("missing_prefix"), /id must match/],
  ["mcp stdio rejected", () => McpServer.remote({ name: "stdio", url: "stdio://server", transport: "stdio" }), /stdio/i],
  ["proxy empty methods", () => ProxyEndpoint.none({
    name: "proxy",
    baseUrl: "https://example.test",
    allowMethods: [],
    allowPathPrefixes: ["/"]
  }), /allowMethods/],
  ["proxy bad response mode", () => ProxyEndpoint.none({
    name: "proxy",
    baseUrl: "https://example.test",
    allowMethods: ["GET"],
    allowPathPrefixes: ["/"],
    responseMode: "json"
  }), /responseMode/],
  ["secret empty value", () => Secret.value(""), /required|non-empty string/],
  ["secret bad ref", () => Secret.ref("bad handle"), /handle must match/]
];

for (const [label, fn, pattern] of builderCases) {
  messages.push(await expectReject(label, fn, pattern));
}

const invalidNames = ["", "UPPER", "two words", "-starts-bad", "a".repeat(129), "slashes/name", "name.with.dot"];
for (const name of invalidNames) {
  await expectReject("skill-name fuzz " + JSON.stringify(name), () => Skill.fromFiles({
    name,
    files: { "SKILL.md": "# x" }
  }), /name must match/);
}

const invalidPromptValues = [null, 0, {}, ["ok", 1], ["ok", null]];
for (const prompt of invalidPromptValues) {
  await expectReject("prompt fuzz " + JSON.stringify(prompt), () => client.submit({
    model: "claude-haiku-4-5",
    prompt,
    secrets: { apiKeys: { anthropic: "sk-ant" } }
  }), /prompt/);
}
strictEqual(calls.length, 0, "fuzzed invalid inputs must not make HTTP calls");

console.log(JSON.stringify({
  ok: true,
  submitRejects: submitCases.length,
  builderRejects: builderCases.length,
  fuzzRejects: invalidNames.length + invalidPromptValues.length,
  calls: calls.length,
  sampleMessage: messages[0]
}));
`;
    const result = await runChild(script, "sdk-submit-user-inputs-invalid.mjs", 120_000);
    expect(result).toMatchObject({
      ok: true,
      submitRejects: 22,
      builderRejects: 18,
      fuzzRejects: 12,
      calls: 0
    });
  });

  it("covers prompt/provider/outputs extremes and parent-run key inheritance", async () => {
    const script = CHILD_HARNESS + String.raw`
const { AgentExecutor, Models, RuntimeSizes } = await import("@aexhq/sdk");
const { calls, fetch } = makeFetch();
const client = new AgentExecutor({
  apiToken: "aex_extreme_inputs_token",
  baseUrl: "https://example.invalid",
  fetch
});

const longPrompt = "x".repeat(32 * 1024);
await client.submit({
  provider: "deepseek",
  model: "deepseek-v4-flash",
  prompt: longPrompt,
  outputs: {
    allowedDirs: ["", "/workspace/one", "/workspace/two", ""],
    deniedDirs: ["", "/workspace/two/tmp"],
    captureTimeoutMs: 1,
    maxFileBytes: 0,
    maxTotalBytes: Number.MAX_SAFE_INTEGER,
    maxFiles: 100000
  },
  secrets: { apiKeys: { deepseek: "sk-deepseek" } },
  idempotencyKey: "idem-extreme-outputs"
});
let body = onlyRunBody(calls);
strictEqual(body.provider, "deepseek");
strictEqual(body.submission.prompt[0].length, 32 * 1024);
deepStrictEqual(body.submission.outputs, {
  allowedDirs: ["/workspace/one", "/workspace/two"],
  deniedDirs: ["/workspace/two/tmp"],
  captureTimeoutMs: 1,
  maxFileBytes: 0,
  maxTotalBytes: Number.MAX_SAFE_INTEGER,
  maxFiles: 100000
});

resetCalls(calls);
await client.submit({
  model: Models.CLAUDE_HAIKU_4_5,
  prompt: "child run inherits provider credentials",
  parentRunId: "run_parent_without_keys",
  runtimeSize: RuntimeSizes.SHARED_0_06X_256MB,
  idempotencyKey: "idem-parent-inherits"
});
body = onlyRunBody(calls);
strictEqual(body.provider, "anthropic");
strictEqual(body.parentRunId, "run_parent_without_keys");
strictEqual(body.runtimeSize, "shared-0.06x-256mb");
deepStrictEqual(body.secrets, {});

resetCalls(calls);
await client.submit({
  provider: "openrouter",
  model: Models.GPT_4O_MINI,
  prompt: ["multi-provider model", "non-default provider"],
  secrets: { apiKeys: { openrouter: "sk-openrouter" } },
  includeBuiltinTools: true,
  idempotencyKey: "idem-openrouter"
});
body = onlyRunBody(calls);
strictEqual(body.provider, "openrouter");
strictEqual(body.submission.model, "gpt-4o-mini");
deepStrictEqual(body.submission.prompt, ["multi-provider model", "non-default provider"]);
strictEqual(body.submission.includeBuiltinTools, true);

resetCalls(calls);
await client.submit({
  model: "claude-haiku-4-5",
  prompt: "empty optional shapes normalize away",
  outputs: { allowedDirs: [""], deniedDirs: [""] },
  postHook: { command: "   " },
  limits: {},
  secrets: { apiKeys: { anthropic: "sk-ant" } },
  idempotencyKey: "idem-normalize-empty"
});
body = onlyRunBody(calls);
ok(!("outputs" in body.submission));
ok(!("postHook" in body));
ok(!("limits" in body));

console.log(JSON.stringify({
  ok: true,
  finalProvider: body.provider,
  submitCalls: runBodies(calls).length
}));
`;
    const result = await runChild(script, "sdk-submit-user-inputs-extremes.mjs");
    expect(result).toMatchObject({
      ok: true,
      finalProvider: "anthropic",
      submitCalls: 1
    });
  });
});
