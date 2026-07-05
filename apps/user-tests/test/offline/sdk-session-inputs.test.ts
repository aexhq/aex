/**
 * Blackbox SDK session-input coverage through a clean installed package.
 *
 * The one-shot `submit` surface folded into sessions: a run is now
 * `sessions.create(...)` (config only) + `session.send(...)` / `sessions.run(...)`
 * (the message). These cases intentionally run in child processes whose cwd is
 * the user-test install tempdir. That keeps the assertions at the user boundary:
 * `import "@aexhq/sdk"` resolves from the packed or published artifact, while a
 * fake fetch captures the exact SDK wire request (POST /api/sessions) without
 * dispatching a live run.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

const CHILD_HARNESS = String.raw`
import { deepStrictEqual, match, ok, strictEqual } from "node:assert/strict";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

async function importSdk() {
  const sdk = await import("@aexhq/sdk");
  ok(!("Tools" in sdk), "legacy Tools namespace must not be exported");
  return sdk;
}

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
  let sessionCounter = 0;
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

    if (url.includes("/api/skills/") && method === "PUT") {
      const name = decodeURIComponent(url.split("/api/skills/")[1] ?? "");
      return new Response(JSON.stringify({
        skill: {
          kind: "skill",
          name,
          contentHash: body && typeof body.contentHash === "string" ? body.contentHash : "sha256:" + "a".repeat(64),
          description: body && typeof body.description === "string" ? body.description : "",
          sizeBytes: body && typeof body.sizeBytes === "number" ? body.sizeBytes : 0,
          version: 1
        },
        updated: true
      }), { status: 200, headers: { "content-type": "application/json" } });
    }

    if (url.endsWith("/api/sessions") && method === "POST") {
      sessionCounter += 1;
      return new Response(JSON.stringify({
        session: {
          id: "sess_user_input_" + sessionCounter,
          workspaceId: "ws_user_inputs",
          status: "idle",
          turnSeq: 0,
          createdAt: new Date(0).toISOString()
        }
      }), { status: 201, headers: { "content-type": "application/json" } });
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

function createBodies(calls) {
  return calls
    .filter((call) => call.method === "POST" && call.url.endsWith("/api/sessions"))
    .map((call) => call.body);
}

function onlyCreateBody(calls) {
  const bodies = createBodies(calls);
  strictEqual(bodies.length, 1, "expected exactly one session-create call");
  return bodies[0];
}

function onlyCreateCall(calls) {
  const matches = calls.filter((call) => call.method === "POST" && call.url.endsWith("/api/sessions"));
  strictEqual(matches.length, 1, "expected exactly one session-create call");
  return matches[0];
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

function upsertSkillCalls(calls) {
  return calls.filter((call) => call.method === "PUT" && call.url.includes("/api/skills/"));
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

// Skills are first-class session inputs: build one from a temp directory
// containing a SKILL.md whose YAML frontmatter carries the skill name +
// description, then pass the result via Skill.fromDir and top-level skills.
function makeSkillDir(name, description) {
  const dir = mkdtempSync(join(tmpdir(), "aex-skill-"));
  writeFileSync(
    join(dir, "SKILL.md"),
    "---\nname: " + name + "\ndescription: " + description + "\n---\n# " + name + "\n" + description + "\n"
  );
  return dir;
}

function skillEntries(body) {
  return body.submission.skills ?? [];
}
`;

describe("SDK session inputs (installed package)", () => {
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

  it("serializes every major session input into the public /api/sessions wire request", async () => {
    const script = CHILD_HARNESS + String.raw`
const {
  Aex,
  AgentsMd,
  BuiltinTools,
  File,
  McpServer,
  Secret,
  Sizes,
  Tool,
  Skill
} = await importSdk();

const { calls, fetch } = makeFetch();
const client = new Aex({
  apiKey: "aex_user_inputs_token",
  baseUrl: "https://example.invalid",
  fetch
});

const skill = await Skill.fromDir(makeSkillDir("alpha-skill", "Use the alpha behavior."), { name: "alpha-skill" });
const skillHash = skill.ref.contentHash;
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

const session = await client.sessions.create({
  model: "claude-haiku-4-5",
  system: "System instructions.",
  tools: [BuiltinTools.web_fetch, BuiltinTools.web_fetch, tool],
  skills: [skill],
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
  environment: {
    networking: { mode: "limited", allowedHosts: ["api.example.test"] },
    variables: { PLAIN_ENV: "visible" },
    packages: [{ name: "apt:jq" }, { name: "pip:pandas" }],
    secrets: {
      EPHEMERAL_TOKEN: Secret.value("ephemeral-secret-value"),
      WORKSPACE_TOKEN: Secret.ref("workspace-secret")
    }
  },
  metadata: { suite: "sdk-session-inputs", nested: { count: 2 } },
  runtime: Sizes.SHARED_2X_8GB,
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
  apiKeys: { anthropic: "sk-ant-user-inputs" },
  idempotencyKey: "idem-user-inputs",
  webhook: { url: "https://hooks.example.test/aex" },
  overrides: { timeout: "90m", idleTtl: "5m", maxSpendUsd: 3.5 }
});

strictEqual(session.id, "sess_user_input_1");
strictEqual(presignCalls(calls).length, 4);
strictEqual(storagePuts(calls).length, 4);
strictEqual(finalizeCalls(calls).length, 4);
strictEqual(upsertSkillCalls(calls).length, 1);
const skillUpsert = upsertSkillCalls(calls)[0].body;
strictEqual(skillUpsert.contentHash, skillHash);
strictEqual(skillUpsert.description, "Use the alpha behavior.");
strictEqual(typeof skillUpsert.sizeBytes, "number");
ok(skillUpsert.sizeBytes > 0);

const create = onlyCreateCall(calls);
strictEqual(create.headers["idempotency-key"], "idem-user-inputs");
const body = create.body;
strictEqual(body.provider, "anthropic");
strictEqual(body.runtimeSize, "shared-2x-8gb");
strictEqual(body.timeout, "90m");
deepStrictEqual(body.retention, { idleTtl: "5m" });
deepStrictEqual(body.limits, { maxSpendUsd: 3.5 });
deepStrictEqual(body.webhook, { url: "https://hooks.example.test/aex" });
ok(!("parentRunId" in body));
ok(!("postHook" in body));

const submission = body.submission;
strictEqual(submission.model, "claude-haiku-4-5");
strictEqual(submission.system, "System instructions.");
ok(!("prompt" in submission));
deepStrictEqual(submission.metadata, { suite: "sdk-session-inputs", nested: { count: 2 } });
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
deepStrictEqual(submission.skills, [{ kind: "skill", name: "alpha-skill" }]);
strictEqual(submission.tools[0], "web_fetch");
strictEqual(submission.tools.length, 2, "deduped builtin + custom tool");
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

ok(!("proxyEndpoints" in body));
deepStrictEqual(body.secrets.apiKeys, { anthropic: "sk-ant-user-inputs" });
deepStrictEqual(body.secrets.envSecrets, { EPHEMERAL_TOKEN: "ephemeral-secret-value" });
deepStrictEqual(body.secrets.mcpServers, [
  { name: "docs", url: "https://mcp.example.test/sse", headers: { Authorization: "Bearer mcp-secret" } }
]);
ok(!("proxyEndpointAuth" in body.secrets));

console.log(JSON.stringify({
  ok: true,
  createCalls: createBodies(calls).length,
  assetUploads: presignCalls(calls).length,
  skillNames: skillEntries(body).map((entry) => entry.name),
  toolEntries: submission.tools.length
}));
`;
    const result = await runChild(script, "sdk-session-inputs-full.mjs");
    expect(result).toMatchObject({
      ok: true,
      createCalls: 1,
      assetUploads: 4,
      skillNames: ["alpha-skill"],
      toolEntries: 2
    });
  });

  it("covers skill-tool absence, bad tool entries, ordering, reuse, and many-skill stress", async () => {
    const script = CHILD_HARNESS + String.raw`
const { Aex, Skill } = await importSdk();

function makeClient() {
  const harness = makeFetch();
  const client = new Aex({
    apiKey: "aex_skill_inputs_token",
    baseUrl: "https://example.invalid",
    fetch: harness.fetch
  });
  return { ...harness, client };
}

const none = makeClient();
await none.client.sessions.create({
  model: "claude-haiku-4-5",
  apiKeys: { anthropic: "sk-ant" }
});
deepStrictEqual(onlyCreateBody(none.calls).submission.tools, []);
ok(!("skills" in onlyCreateBody(none.calls)));
strictEqual(presignCalls(none.calls).length, 0);

const fake = makeClient();
await expectReject("fake tool object", () => fake.client.sessions.create({
  model: "claude-haiku-4-5",
  tools: [{ kind: "workspace", id: "skill_does_not_exist" }],
  apiKeys: { anthropic: "sk-ant" }
}), /tools\[0\] must be a Tool or a builtin tool name/);
strictEqual(fake.calls.length, 0);

const staleSkillUse = makeClient();
const staleSkill = await Skill.fromDir(makeSkillDir("stale-tool-skill", "Must use skills option."), { name: "stale-tool-skill" });
await expectReject("skill in tools array", () => staleSkillUse.client.sessions.create({
  model: "claude-haiku-4-5",
  tools: [staleSkill],
  apiKeys: { anthropic: "sk-ant" }
}), /tools\[0\] is a Skill; pass skills via the top-level skills option/);
strictEqual(staleSkillUse.calls.length, 0);

const order = makeClient();
const orderedSkills = [];
for (let i = 0; i < 6; i += 1) {
  orderedSkills.push(await Skill.fromDir(
    makeSkillDir("ordered-skill-" + i, "Reply with order " + i + "."),
    { name: "ordered-skill-" + i }
  ));
}
await order.client.sessions.create({
  model: "claude-haiku-4-5",
  skills: orderedSkills,
  apiKeys: { anthropic: "sk-ant" }
});
deepStrictEqual(
  skillEntries(onlyCreateBody(order.calls)).map((entry) => entry.name),
  orderedSkills.map((skill) => skill.ref.name)
);
strictEqual(presignCalls(order.calls).length, 6);
strictEqual(upsertSkillCalls(order.calls).length, 6);

const repeated = makeClient();
const reusable = await Skill.fromDir(
  makeSkillDir("reusable-skill", "Apply this every time."),
  { name: "reusable-skill" }
);
await expectReject("duplicate skill names", () => repeated.client.sessions.create({
  model: "claude-haiku-4-5",
  skills: [reusable, reusable],
  apiKeys: { anthropic: "sk-ant" }
}), /skills duplicate name: reusable-skill/);
strictEqual(repeated.calls.length, 0);

await repeated.client.sessions.create({
  model: "claude-haiku-4-5",
  skills: [reusable],
  apiKeys: { anthropic: "sk-ant" }
});
deepStrictEqual(skillEntries(onlyCreateBody(repeated.calls)), [
  { kind: "skill", name: "reusable-skill" }
]);
strictEqual(presignCalls(repeated.calls).length, 1);
strictEqual(upsertSkillCalls(repeated.calls).length, 1);

resetCalls(repeated.calls);
await repeated.client.sessions.create({
  model: "claude-haiku-4-5",
  skills: [reusable],
  apiKeys: { anthropic: "sk-ant" }
});
deepStrictEqual(skillEntries(onlyCreateBody(repeated.calls)), [
  { kind: "skill", name: "reusable-skill" }
]);
strictEqual(presignCalls(repeated.calls).length, 0);
strictEqual(upsertSkillCalls(repeated.calls).length, 0);

const stress = makeClient();
const many = [];
for (let i = 0; i < 64; i += 1) {
  const name = "stress-skill-" + String(i).padStart(2, "0");
  many.push(await Skill.fromDir(makeSkillDir(name, "Return token " + i + "."), { name }));
}
await stress.client.sessions.create({
  model: "claude-haiku-4-5",
  skills: many,
  apiKeys: { anthropic: "sk-ant" }
});
const stressSkills = skillEntries(onlyCreateBody(stress.calls));
strictEqual(stressSkills.length, 64);
strictEqual(stressSkills[0].name, "stress-skill-00");
strictEqual(stressSkills[63].name, "stress-skill-63");
deepStrictEqual(stressSkills.map((entry) => entry.name), many.map((skill) => skill.ref.name));
strictEqual(presignCalls(stress.calls).length, 64);
strictEqual(storagePuts(stress.calls).length, 64);
strictEqual(finalizeCalls(stress.calls).length, 64);
strictEqual(upsertSkillCalls(stress.calls).length, 64);

console.log(JSON.stringify({
  ok: true,
  orderedCount: orderedSkills.length,
  stressCount: stressSkills.length,
  repeatedUploadCount: presignCalls(repeated.calls).length,
  fakeCalls: fake.calls.length,
  staleSkillCalls: staleSkillUse.calls.length
}));
`;
    const result = await runChild(script, "sdk-session-inputs-skills.mjs", 180_000);
    expect(result).toMatchObject({
      ok: true,
      orderedCount: 6,
      stressCount: 64,
      repeatedUploadCount: 0,
      fakeCalls: 0,
      staleSkillCalls: 0
    });
  });

  it("rejects invalid session input and primitive-builder edge cases before posting", async () => {
    const script = CHILD_HARNESS + String.raw`
const {
  Aex,
  AgentsMd,
  File,
  McpServer,
  Secret,
  Tool,
  Skill
} = await importSdk();

const { calls, fetch } = makeFetch();
const client = new Aex({
  apiKey: "aex_invalid_inputs_token",
  baseUrl: "https://example.invalid",
  fetch
});
const validCreate = {
  model: "claude-haiku-4-5",
  apiKeys: { anthropic: "sk-ant" }
};

// Session-create (openSession) rejections — legacy submit fields are gone,
// composition instances are validated at the SDK boundary.
const createCases = [
  ["missing options", () => client.openSession(undefined), /options is required/],
  ["missing provider key", () => client.openSession({ model: "claude-haiku-4-5" }), /provider API key is required/],
  ["provider mismatch", () => client.openSession({
    model: "gpt-4.1",
    provider: "anthropic",
    apiKeys: { anthropic: "sk-ant" }
  }), /provider "anthropic" is not available/],
  ["removed prompt", () => client.openSession({ ...validCreate, prompt: "hello" }), /prompt is not a supported option/],
  ["removed secrets", () => client.openSession({ ...validCreate, secrets: { apiKeys: { anthropic: "sk-ant" } } }), /secrets is not a supported option/],
  ["removed secretEnv", () => client.openSession({ ...validCreate, secretEnv: { X: Secret.value("v") } }), /secretEnv is not a supported option/],
  ["removed runtimeSize", () => client.openSession({ ...validCreate, runtimeSize: "shared-2x-8gb" }), /runtimeSize is not a supported option/],
  ["removed timeout", () => client.openSession({ ...validCreate, timeout: "15m" }), /timeout is not a supported option/],
  ["removed limits", () => client.openSession({ ...validCreate, limits: { maxConcurrentChildRuns: 4 } }), /limits is not a supported option/],
  ["removed parentRunId", () => client.openSession({ ...validCreate, parentRunId: "run_parent" }), /parentRunId is not a supported option/],
  ["removed postHook", () => client.openSession({ ...validCreate, postHook: { command: "bun test" } }), /postHook is not a supported option/],
  ["removed instructions", () => client.openSession({ ...validCreate, instructions: "be brief" }), /instructions is not a supported option/],
  ["bad skill entry", () => client.openSession({ ...validCreate, skills: [{}] }), /skills\[0\] must be a Skill/],
  ["bad tool object entry", () => client.openSession({ ...validCreate, tools: [{}] }), /tools\[0\] must be a Tool or a builtin tool name/],
  ["bad tool entry", () => client.openSession({ ...validCreate, tools: ["definitely_not_builtin"] }), /not a builtin tool name/],
  ["bad agentsMd entry", () => client.openSession({ ...validCreate, agentsMd: [{}] }), /agentsMd\[0\] must be an AgentsMd instance/],
  ["bad file entry", () => client.openSession({ ...validCreate, files: [{}] }), /files\[0\] must be a File instance/],
  ["bad mcp entry", () => client.openSession({ ...validCreate, mcpServers: [{}] }), /mcpServers\[0\] must be an McpServer instance/],
  ["removed proxyEndpoints", () => client.openSession({ ...validCreate, proxyEndpoints: [{}] }), /proxyEndpoints is not a supported option/],
  ["bad env secret name", () => client.openSession({ ...validCreate, environment: { secrets: { "bad-name": Secret.value("secret") } } }), /env var name/],
  ["bad env secret value", () => client.openSession({ ...validCreate, environment: { secrets: { VALID_NAME: "secret" } } }), /must be a Secret/]
];

const messages = [];
for (const [label, fn, pattern] of createCases) {
  messages.push(await expectReject(label, fn, pattern));
}

// Message (send / run) rejections — the first-turn text is validated before
// the session is created.
const validRun = { model: "claude-haiku-4-5", apiKeys: { anthropic: "sk-ant" } };
const runCases = [
  ["empty message string", () => client.sessions.run({ ...validRun, message: "" }), /message must be a non-empty string/],
  ["empty message array", () => client.sessions.run({ ...validRun, message: [] }), /message must be a non-empty string or string array/],
  ["empty message segment", () => client.sessions.run({ ...validRun, message: ["ok", ""] }), /message segments must be non-empty strings/]
];
for (const [label, fn, pattern] of runCases) {
  messages.push(await expectReject(label, fn, pattern));
}
strictEqual(calls.length, 0, "invalid session inputs must not make HTTP calls");

// Skill-tools are built from a local SKILL.md directory now; cover the
// name/description validation surface from skill-tool.ts.
const validSkillDir = makeSkillDir("valid-name", "A valid skill.");
const noSkillMdDir = mkdtempSync(join(tmpdir(), "aex-skill-"));
writeFileSync(join(noSkillMdDir, "README.md"), "not a skill\n");
const noDescriptionDir = mkdtempSync(join(tmpdir(), "aex-skill-"));
writeFileSync(join(noDescriptionDir, "SKILL.md"), "---\nname: valid-name\n---\n# x\n");
const oversizedDescriptionDir = mkdtempSync(join(tmpdir(), "aex-skill-"));
writeFileSync(
  join(oversizedDescriptionDir, "SKILL.md"),
  "---\nname: valid-name\ndescription: " + "d".repeat(2049) + "\n---\n# x\n"
);

const builderCases = [
  ["skill bad name", () => Skill.fromDir(validSkillDir, { name: "Bad Name" }), /must match/],
  ["skill missing SKILL.md", () => Skill.fromDir(noSkillMdDir), /must contain a SKILL\.md/],
  ["skill missing description", () => Skill.fromDir(noDescriptionDir), /description is required/],
  ["skill oversized description", () => Skill.fromDir(oversizedDescriptionDir), /description must be <= 2048 chars/],
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
  ["secret empty value", () => Secret.value(""), /required|non-empty string/],
  ["secret bad ref", () => Secret.ref("bad handle"), /handle must match/]
];

for (const [label, fn, pattern] of builderCases) {
  messages.push(await expectReject(label, fn, pattern));
}

const invalidNames = ["", "UPPER", "two words", "-starts-bad", "a".repeat(129), "slashes/name", "name.with.dot"];
for (const name of invalidNames) {
  await expectReject(
    "skill-name fuzz " + JSON.stringify(name),
    () => Skill.fromDir(validSkillDir, { name }),
    /must match|name is required/
  );
}

const invalidMessageValues = [null, 0, {}, ["ok", 1], ["ok", null]];
for (const message of invalidMessageValues) {
  await expectReject("message fuzz " + JSON.stringify(message), () => client.sessions.run({
    model: "claude-haiku-4-5",
    apiKeys: { anthropic: "sk-ant" },
    message
  }), /message/);
}
strictEqual(calls.length, 0, "fuzzed invalid inputs must not make HTTP calls");

console.log(JSON.stringify({
  ok: true,
  createRejects: createCases.length,
  runRejects: runCases.length,
  builderRejects: builderCases.length,
  fuzzRejects: invalidNames.length + invalidMessageValues.length,
  calls: calls.length,
  sampleMessage: messages[0]
}));
`;
    const result = await runChild(script, "sdk-session-inputs-invalid.mjs", 120_000);
    expect(result).toMatchObject({
      ok: true,
      createRejects: 21,
      runRejects: 3,
      builderRejects: 16,
      fuzzRejects: 12,
      calls: 0
    });
  });

  it("covers provider/outputs extremes and empty-shape normalization", async () => {
    const script = CHILD_HARNESS + String.raw`
const { Aex, Models, Sizes } = await importSdk();
const { calls, fetch } = makeFetch();
const client = new Aex({
  apiKey: "aex_extreme_inputs_token",
  baseUrl: "https://example.invalid",
  fetch
});

await client.sessions.create({
  provider: "deepseek",
  model: "deepseek-v4-flash",
  outputs: {
    allowedDirs: ["", "/workspace/one", "/workspace/two", ""],
    deniedDirs: ["", "/workspace/two/tmp"],
    captureTimeoutMs: 1,
    maxFileBytes: 0,
    maxTotalBytes: Number.MAX_SAFE_INTEGER,
    maxFiles: 100000
  },
  apiKeys: { deepseek: "sk-deepseek" },
  idempotencyKey: "idem-extreme-outputs"
});
let body = onlyCreateBody(calls);
strictEqual(body.provider, "deepseek");
deepStrictEqual(body.submission.outputs, {
  allowedDirs: ["/workspace/one", "/workspace/two"],
  deniedDirs: ["/workspace/two/tmp"],
  captureTimeoutMs: 1,
  maxFileBytes: 0,
  maxTotalBytes: Number.MAX_SAFE_INTEGER,
  maxFiles: 100000
});

resetCalls(calls);
await client.sessions.create({
  model: Models.CLAUDE_HAIKU_4_5,
  runtime: Sizes.SHARED_0_06X_256MB,
  apiKeys: { anthropic: "sk-ant" },
  idempotencyKey: "idem-default-provider"
});
body = onlyCreateBody(calls);
strictEqual(body.provider, "anthropic");
strictEqual(body.runtimeSize, "shared-0.06x-256mb");
deepStrictEqual(body.secrets, { apiKeys: { anthropic: "sk-ant" } });

resetCalls(calls);
await client.sessions.create({
  provider: "openrouter",
  model: Models.GPT_4O_MINI,
  apiKeys: { openrouter: "sk-openrouter" },
  includeBuiltinTools: true,
  idempotencyKey: "idem-openrouter"
});
body = onlyCreateBody(calls);
strictEqual(body.provider, "openrouter");
strictEqual(body.submission.model, "gpt-4o-mini");
strictEqual(body.submission.includeBuiltinTools, true);

resetCalls(calls);
await client.sessions.create({
  model: "claude-haiku-4-5",
  outputs: { allowedDirs: [""], deniedDirs: [""] },
  apiKeys: { anthropic: "sk-ant" },
  idempotencyKey: "idem-normalize-empty"
});
body = onlyCreateBody(calls);
ok(!("outputs" in body.submission));
ok(!("postHook" in body));
ok(!("limits" in body));

console.log(JSON.stringify({
  ok: true,
  finalProvider: body.provider,
  createCalls: createBodies(calls).length
}));
`;
    const result = await runChild(script, "sdk-session-inputs-extremes.mjs");
    expect(result).toMatchObject({
      ok: true,
      finalProvider: "anthropic",
      createCalls: 1
    });
  });
});
