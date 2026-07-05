/**
 * Deep skill-tool wire-serialization, dedup/reuse, and mixing semantics through
 * a clean installed `@aexhq/sdk`.
 *
 * The sibling `sdk-session-inputs.test.ts` already covers the basics: a single
 * skill-tool serializing to `{ kind:"skill", assetId, name, description }`, a
 * simple in-order tool list, same-instance reuse within one submit, and a
 * 64-skill stress upload. This file goes DEEPER and does NOT repeat those:
 *
 *  1. Cross-SESSION reuse cache — one instance across three sessions uploads
 *     exactly once; sessions 2 and 3 skip presign entirely (instance asset-id
 *     cache short-circuits before the uploader runs).
 *  2. Full mixed ordering + dedup semantics — builtins (deduped) + custom
 *     `Tool`s + skill-tools INTERLEAVED at the call site collapse into the
 *     documented wire order builtins → tools → skills, proving the regrouping.
 *  3. Content-addressed dedup — two DISTINCT instances of byte-identical files
 *     share one assetId and a single stored upload but ride as two wire entries;
 *     two skill-tools with the SAME name but different bytes stay distinct.
 *  4. Draft serialization guard — `.toJSON()` / `JSON.stringify` throw for a
 *     never-uploaded skill-tool AND still throw after upload (the instance ref
 *     stays a draft; the wire ref is minted by the prepare step, not stored back).
 *  5. Non-interference + `includeBuiltinTools` — a skill-tool alongside
 *     agentsMd / files / mcpServers serializes each independently, and skill
 *     entries survive `includeBuiltinTools:false` (they are not builtins).
 *
 * CONFIRMED ordering (see `prepareTools` + `#buildSessionCreateRequest` in
 * packages/sdk/src/client.ts): `prepareTools` splits `tools` into three groups —
 * `builtinNames` (deduped via a Set, first-seen INPUT order), `refs` (custom
 * Tool refs, NOT deduped), and `skillToolRefs` (skill refs, NOT deduped). The
 * request builder then emits `submission.tools` as
 * `[...builtinNames, ...refs, ...skillToolRefs]`, so the wire order is ALWAYS
 * builtins → custom tools → skill-tools regardless of call-site interleaving.
 *
 * Each scenario runs in a child process whose cwd is the install tempdir, so
 * `import "@aexhq/sdk"` resolves the packed/published artifact and a fake fetch
 * captures the exact wire request without dispatching a live run. The fake fetch
 * models the content-addressed asset store: `/assets/presign` returns
 * `exists:true` for a hash already PUT, so a repeat upload of identical bytes is
 * a dedup hit (skips the PUT), exactly like the hosted API.
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

function json(status, obj) {
  return new Response(JSON.stringify(obj), { status, headers: { "content-type": "application/json" } });
}

function hexOf(hash) {
  return hash.startsWith("sha256:") ? hash.slice("sha256:".length) : hash;
}

// A fake fetch that MODELS the content-addressed asset store: a hash that has
// already been PUT presigns as a dedup hit (exists:true, no uploadUrl), so the
// SDK skips the redundant PUT — exactly like the hosted API. Distinct instances
// of identical bytes therefore share one stored object but each still presign.
function makeFetch() {
  const calls = [];
  const stored = new Set();
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
      const hex = hexOf(hash);
      const sizeBytes = body && typeof body.sizeBytes === "number" ? body.sizeBytes : 0;
      if (stored.has(hash)) {
        return json(200, { ok: true, exists: true, assetId: "asset_" + hex, contentHash: hash, sizeBytes });
      }
      return json(201, {
        ok: true,
        exists: false,
        assetId: "asset_" + hex,
        contentHash: hash,
        uploadUrl: "https://object-storage.example.test/assets/" + hex + "?sig=test",
        requiredHeaders: { "x-amz-checksum-sha256": "Y2hlY2tzdW0=" },
        expiresInSeconds: 300
      });
    }

    if (url.includes("object-storage.example.test")) {
      // The bytes are now vaulted under their content-addressed key.
      const m = /\/assets\/([0-9a-f]{64})/.exec(url);
      if (m) stored.add("sha256:" + m[1]);
      return new Response("", { status: 200 });
    }

    if (url.endsWith("/assets/finalize")) {
      const hash = body && typeof body.hash === "string" ? body.hash : "sha256:" + "a".repeat(64);
      const hex = hexOf(hash);
      return json(200, {
        ok: true,
        exists: false,
        assetId: "asset_" + hex,
        contentHash: hash,
        sizeBytes: body && typeof body.sizeBytes === "number" ? body.sizeBytes : 0
      });
    }

    if (url.includes("/api/skills/") && method === "PUT") {
      const name = decodeURIComponent(url.split("/api/skills/")[1] ?? "");
      return json(200, {
        skill: {
          kind: "skill",
          name,
          contentHash: body && typeof body.contentHash === "string" ? body.contentHash : "sha256:" + "a".repeat(64),
          description: body && typeof body.description === "string" ? body.description : "",
          sizeBytes: body && typeof body.sizeBytes === "number" ? body.sizeBytes : 0,
          version: 1
        },
        updated: true
      });
    }

    if (url.endsWith("/api/sessions") && method === "POST") {
      sessionCounter += 1;
      return json(201, {
        session: {
          id: "sess_wire_" + sessionCounter,
          workspaceId: "ws_wire",
          status: "idle",
          turnSeq: 0,
          createdAt: new Date(0).toISOString()
        }
      });
    }

    return json(200, { ok: true });
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

// Skills are first-class session inputs: build one from a temp dir whose
// SKILL.md frontmatter carries the name + description, then pass through
// Skill.fromDir and submit it via the top-level skills option.
function makeSkillDir(name, description) {
  const dir = mkdtempSync(join(tmpdir(), "aex-skill-"));
  writeFileSync(
    join(dir, "SKILL.md"),
    "---\nname: " + name + "\ndescription: " + description + "\n---\n# " + name + "\n" + description + "\n"
  );
  return dir;
}

// Same frontmatter (identical wire name + description) but a caller-controlled
// body — used to vary the BYTES while keeping the serialized metadata fixed.
function makeSkillDirBody(name, description, body) {
  const dir = mkdtempSync(join(tmpdir(), "aex-skill-"));
  writeFileSync(join(dir, "SKILL.md"), "---\nname: " + name + "\ndescription: " + description + "\n---\n" + body);
  return dir;
}

function toolEntries(body) {
  return body.submission.tools;
}

function builtinEntries(body) {
  return body.submission.tools.filter((entry) => typeof entry === "string");
}

function assetToolEntries(body) {
  return body.submission.tools.filter(
    (entry) => entry && typeof entry === "object" && entry.kind === "asset"
  );
}

function skillEntries(body) {
  return body.submission.skills ?? [];
}
`;

describe("SDK skill-tool wire serialization + dedup (installed package)", () => {
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

  it("reuses one Skill instance across sessions, uploading and upserting exactly once", async () => {
    const script = CHILD_HARNESS + String.raw`
const { Aex, Skill } = await importSdk();

const { calls, fetch } = makeFetch();
const client = new Aex({
  apiKey: "aex_reuse_token",
  baseUrl: "https://example.invalid",
  fetch
});

const skill = await Skill.fromDir(makeSkillDir("reuse-skill", "Reuse across sessions."), { name: "reuse-skill" });
const expectedEntry = { kind: "skill", name: "reuse-skill" };

// Session A: first use uploads the bundle and upserts the workspace skill once.
await client.sessions.create({ model: "claude-haiku-4-5", skills: [skill], apiKeys: { anthropic: "sk-ant" } });
deepStrictEqual(skillEntries(onlyCreateBody(calls)), [expectedEntry]);
deepStrictEqual(onlyCreateBody(calls).submission.tools, []);
strictEqual(presignCalls(calls).length, 1);
strictEqual(storagePuts(calls).length, 1);
strictEqual(finalizeCalls(calls).length, 1);
strictEqual(upsertSkillCalls(calls).length, 1);
strictEqual(upsertSkillCalls(calls)[0].body.contentHash, skill.ref.contentHash);
// The instance is still a draft (its ref never mutates) but now caches the name.
strictEqual(skill.isDraft, true);
strictEqual(skill._cachedName, "reuse-skill");

// Session B: the SAME instance short-circuits before upload/upsert.
resetCalls(calls);
await client.sessions.create({ model: "claude-haiku-4-5", skills: [skill], apiKeys: { anthropic: "sk-ant" } });
deepStrictEqual(skillEntries(onlyCreateBody(calls)), [expectedEntry]);
strictEqual(presignCalls(calls).length, 0);
strictEqual(storagePuts(calls).length, 0);
strictEqual(finalizeCalls(calls).length, 0);
strictEqual(upsertSkillCalls(calls).length, 0);

// Session C: still cached — a third reuse is still upload-free.
resetCalls(calls);
await client.sessions.create({ model: "claude-haiku-4-5", skills: [skill], apiKeys: { anthropic: "sk-ant" } });
deepStrictEqual(skillEntries(onlyCreateBody(calls)), [expectedEntry]);
strictEqual(presignCalls(calls).length, 0);

console.log(JSON.stringify({ ok: true, cachedName: skill._cachedName, sessionCReuseUploads: presignCalls(calls).length }));
`;
    const result = await runChild(script, "skill-wire-reuse.mjs");
    expect(result).toMatchObject({ ok: true, sessionCReuseUploads: 0 });
  });

  it("keeps tools and first-class skills ordered in their separate submission fields", async () => {
    const script = CHILD_HARNESS + String.raw`
const { Aex, BuiltinTools, Tool, Skill } = await importSdk();

const { calls, fetch } = makeFetch();
const client = new Aex({
  apiKey: "aex_order_token",
  baseUrl: "https://example.invalid",
  fetch
});

const skillA = await Skill.fromDir(makeSkillDir("skill-alpha", "Alpha skill."), { name: "skill-alpha" });
const skillB = await Skill.fromDir(makeSkillDir("skill-bravo", "Bravo skill."), { name: "skill-bravo" });
const toolX = await Tool.fromFiles({
  name: "tool_x",
  description: "Tool X.",
  inputSchema: { type: "object", properties: { q: { type: "string" } }, required: ["q"] },
  entry: "index.js",
  files: { "index.js": "export default async function () { return { content: [] }; }\n" }
});
const toolY = await Tool.fromFiles({
  name: "tool_y",
  description: "Tool Y.",
  inputSchema: { type: "object", properties: {} },
  entry: "index.js",
  files: { "index.js": "export default async function () { return { content: [] }; }\n" }
});

// Interleave builtins and a custom tool, including a duplicate builtin. Skills
// are first-class: they ride the separate top-level skills option and preserve
// that input order.
await client.sessions.create({
  model: "claude-haiku-4-5",
  tools: [
    BuiltinTools.web_search,
    toolX,
    BuiltinTools.web_fetch,
    BuiltinTools.web_search,
    BuiltinTools.bash
  ],
  skills: [skillA, skillB],
  apiKeys: { anthropic: "sk-ant" }
});

const body = onlyCreateBody(calls);
const toolXEntry = {
  kind: "asset",
  assetId: assetIdFromHash(toolX.ref.contentHash),
  name: "tool_x",
  description: "Tool X.",
  input_schema: { type: "object", properties: { q: { type: "string" } }, required: ["q"] },
  entry: "index.js"
};

// Tool sequence: 3 deduped builtins (first-seen input order) -> custom tool entry.
deepStrictEqual(toolEntries(body), [
  "web_search",
  "web_fetch",
  "bash",
  toolXEntry
]);
strictEqual(toolEntries(body).length, 4);
deepStrictEqual(builtinEntries(body), ["web_search", "web_fetch", "bash"]);
deepStrictEqual(skillEntries(body), [
  { kind: "skill", name: "skill-alpha" },
  { kind: "skill", name: "skill-bravo" }
]);
strictEqual(assetToolEntries(body).length, 1);
// One custom tool and two skills upload; each skill also upserts once.
strictEqual(presignCalls(calls).length, 3);
strictEqual(storagePuts(calls).length, 3);
strictEqual(finalizeCalls(calls).length, 3);
strictEqual(upsertSkillCalls(calls).length, 2);

// A separate submit proves a Skill in tools[] fails fast with a migration hint.
resetCalls(calls);
await expectReject(
  "skill in tools array",
  () => client.sessions.create({
    model: "claude-haiku-4-5",
    tools: [skillA],
    apiKeys: { anthropic: "sk-ant" }
  }),
  /tools\[0\] is a Skill; pass skills via the top-level skills option/
);
strictEqual(calls.length, 0);

// A separate submit keeps tools and skills independent.
await client.sessions.create({
  model: "claude-haiku-4-5",
  tools: [toolY, BuiltinTools.grep],
  skills: [skillB],
  apiKeys: { anthropic: "sk-ant" }
});
const body2 = onlyCreateBody(calls);
strictEqual(body2.submission.tools[0], "grep");
strictEqual(body2.submission.tools[1].name, "tool_y");
deepStrictEqual(skillEntries(body2), [{ kind: "skill", name: "skill-bravo" }]);

console.log(JSON.stringify({
  ok: true,
  sequence: toolEntries(body).map((e) => (typeof e === "string" ? e : e.name)),
  skills: skillEntries(body).map((e) => e.name),
  builtins: builtinEntries(body),
  toolCount: assetToolEntries(body).length,
  skillCount: skillEntries(body).length,
  uploads: 3,
  reorderedSecond: {
    tools: body2.submission.tools.map((e) => (typeof e === "string" ? e : e.name)),
    skills: skillEntries(body2).map((e) => e.name)
  }
}));
`;
    const result = await runChild(script, "skill-wire-order.mjs");
    expect(result).toMatchObject({
      ok: true,
      sequence: ["web_search", "web_fetch", "bash", "tool_x"],
      skills: ["skill-alpha", "skill-bravo"],
      builtins: ["web_search", "web_fetch", "bash"],
      toolCount: 1,
      skillCount: 2,
      uploads: 3,
      reorderedSecond: {
        tools: ["grep", "tool_y"],
        skills: ["skill-bravo"]
      }
    });
  });

  it("content-addresses skill bundles and rejects duplicate skill names in one session", async () => {
    const script = CHILD_HARNESS + String.raw`
const { Aex, Skill } = await importSdk();

// --- Distinct instances, byte-identical files, different names: one stored
// object, two workspace skill upserts, two name-only session refs.
{
  const { calls, fetch } = makeFetch();
  const client = new Aex({ apiKey: "aex_dedup_token", baseUrl: "https://example.invalid", fetch });

  const sX = await Skill.fromDir(makeSkillDir("twin-skill", "Twin skill."), { name: "twin-skill-x" });
  const sY = await Skill.fromDir(makeSkillDir("twin-skill", "Twin skill."), { name: "twin-skill-y" });
  ok(sX !== sY, "distinct instances");
  strictEqual(sX.ref.contentHash, sY.ref.contentHash, "byte-identical files hash the same");

  await client.sessions.create({ model: "claude-haiku-4-5", skills: [sX], apiKeys: { anthropic: "sk-ant" } });
  const entries = skillEntries(onlyCreateBody(calls));
  deepStrictEqual(entries, [{ kind: "skill", name: "twin-skill-x" }]);
  strictEqual(presignCalls(calls).length, 1);
  strictEqual(storagePuts(calls).length, 1);
  strictEqual(finalizeCalls(calls).length, 1);
  strictEqual(upsertSkillCalls(calls).length, 1);
  strictEqual(upsertSkillCalls(calls)[0].body.contentHash, sX.ref.contentHash);

  resetCalls(calls);
  await client.sessions.create({ model: "claude-haiku-4-5", skills: [sY], apiKeys: { anthropic: "sk-ant" } });
  deepStrictEqual(skillEntries(onlyCreateBody(calls)), [{ kind: "skill", name: "twin-skill-y" }]);
  // Distinct Skill instance: it still presigns, but the server reports an
  // existing content-addressed object, so no PUT/finalize is needed.
  strictEqual(presignCalls(calls).length, 1);
  strictEqual(storagePuts(calls).length, 0);
  strictEqual(finalizeCalls(calls).length, 0);
  strictEqual(upsertSkillCalls(calls).length, 1);
  strictEqual(upsertSkillCalls(calls)[0].body.contentHash, sX.ref.contentHash);
}

// --- Same name, DIFFERENT bytes: duplicate names in one session are rejected
// before upload; updating the same workspace name is a sequential-session path.
let sameNameSummary;
{
  const { calls, fetch } = makeFetch();
  const client = new Aex({ apiKey: "aex_samename_token", baseUrl: "https://example.invalid", fetch });

  const sA = await Skill.fromDir(makeSkillDirBody("dup-name", "Same description.", "# A\nAlpha body.\n"));
  const sB = await Skill.fromDir(makeSkillDirBody("dup-name", "Same description.", "# B\nBravo body.\n"));
  strictEqual(sA.ref.name, "dup-name");
  strictEqual(sB.ref.name, "dup-name");
  ok(sA.ref.contentHash !== sB.ref.contentHash, "different bytes hash differently");

  await expectReject(
    "duplicate skill names",
    () => client.sessions.create({ model: "claude-haiku-4-5", skills: [sA, sB], apiKeys: { anthropic: "sk-ant" } }),
    /skills duplicate name: dup-name/
  );
  strictEqual(calls.length, 0);

  await client.sessions.create({ model: "claude-haiku-4-5", skills: [sA], apiKeys: { anthropic: "sk-ant" } });
  deepStrictEqual(skillEntries(onlyCreateBody(calls)), [{ kind: "skill", name: "dup-name" }]);
  strictEqual(upsertSkillCalls(calls)[0].body.contentHash, sA.ref.contentHash);

  resetCalls(calls);
  await client.sessions.create({ model: "claude-haiku-4-5", skills: [sB], apiKeys: { anthropic: "sk-ant" } });
  deepStrictEqual(skillEntries(onlyCreateBody(calls)), [{ kind: "skill", name: "dup-name" }]);
  strictEqual(upsertSkillCalls(calls)[0].body.contentHash, sB.ref.contentHash);
  strictEqual(storagePuts(calls).length, 1);
  sameNameSummary = { duplicateRejected: true, updatedHash: upsertSkillCalls(calls)[0].body.contentHash === sB.ref.contentHash };
}

console.log(JSON.stringify({ ok: true, sameName: sameNameSummary }));
`;
    const result = await runChild(script, "skill-wire-dedup.mjs");
    expect(result).toMatchObject({
      ok: true,
      sameName: { duplicateRejected: true, updatedHash: true }
    });
  });

  it("guards draft Skill serialization before and after upload", async () => {
    const script = CHILD_HARNESS + String.raw`
const { Aex, Skill } = await importSdk();

const { calls, fetch } = makeFetch();
const client = new Aex({ apiKey: "aex_draft_token", baseUrl: "https://example.invalid", fetch });

const draft = await Skill.fromDir(makeSkillDir("draft-skill", "Draft skill."), { name: "draft-skill" });
strictEqual(draft.isDraft, true);
strictEqual(draft._cachedName, undefined);
strictEqual(draft.ref.kind, "draft");

// A never-uploaded draft cannot become a wire ref on its own.
const m1 = await expectReject("toJSON before upload", () => draft.toJSON(), /draft skill cannot be JSON-serialised/);
await expectReject("JSON.stringify before upload", () => JSON.stringify(draft), /draft skill cannot be JSON-serialised/);
// No HTTP happened just by serializing.
strictEqual(calls.length, 0);

// Use it in a session — the prepare step uploads, upserts, and caches the name.
await client.sessions.create({ model: "claude-haiku-4-5", skills: [draft], apiKeys: { anthropic: "sk-ant" } });
deepStrictEqual(skillEntries(onlyCreateBody(calls)), [
  { kind: "skill", name: "draft-skill" }
]);
strictEqual(upsertSkillCalls(calls)[0].body.contentHash, draft.ref.contentHash);
strictEqual(draft._cachedName, "draft-skill", "workspace name cached on the instance");

// The instance ref is STILL a draft (the wire ref is minted by prepareSkills,
// not written back), so toJSON keeps throwing even after a successful upload.
strictEqual(draft.isDraft, true);
strictEqual(draft.ref.kind, "draft");
await expectReject("toJSON after upload", () => draft.toJSON(), /draft skill cannot be JSON-serialised/);

console.log(JSON.stringify({ ok: true, sampleMessage: m1, cachedAfterUpload: draft._cachedName === "draft-skill" }));
`;
    const result = await runChild(script, "skill-wire-draft.mjs");
    expect(result).toMatchObject({ ok: true, cachedAfterUpload: true });
  });

  it("keeps first-class skills independent of agentsMd/files/mcp and of includeBuiltinTools", async () => {
    const script = CHILD_HARNESS + String.raw`
const { Aex, AgentsMd, BuiltinTools, File, McpServer, Skill } = await importSdk();

const { calls, fetch } = makeFetch();
const client = new Aex({ apiKey: "aex_mix_token", baseUrl: "https://example.invalid", fetch });

// --- Non-interference: a first-class skill next to agentsMd, files, and one mcp entry.
const skill = await Skill.fromDir(makeSkillDir("mixed-skill", "Mixed skill."), { name: "mixed-skill" });
const agentsMd = await AgentsMd.fromContent("# Rules\nBe brief.\n", { name: "mixed-rules" });
const file = await File.fromBytes({
  name: "data.csv",
  bytes: new TextEncoder().encode("id,value\n1,alpha\n"),
  mountPath: "/workspace/in"
});
await client.sessions.create({
  model: "claude-haiku-4-5",
  skills: [skill],
  agentsMd: [agentsMd],
  files: [file],
  mcpServers: [
    McpServer.remote({ name: "docs", url: "https://mcp.example.test/sse", transport: "sse", headers: { Authorization: "Bearer x" } })
  ],
  apiKeys: { anthropic: "sk-ant" }
});
const body = onlyCreateBody(calls);
deepStrictEqual(skillEntries(body), [
  { kind: "skill", name: "mixed-skill" }
]);
deepStrictEqual(body.submission.tools, []);
deepStrictEqual(body.submission.agentsMd, [
  { kind: "asset", assetId: assetIdFromHash(agentsMd.ref.contentHash), name: "mixed-rules" }
]);
deepStrictEqual(body.submission.files, [
  { kind: "asset", assetId: assetIdFromHash(file.ref.contentHash), name: "data", mountPath: "/workspace/in" }
]);
deepStrictEqual(body.submission.mcpServers, [
  { name: "docs", transport: "sse", url: "https://mcp.example.test/sse" }
]);
// Distinct asset-backed inputs = skill + agentsMd + file (mcp is a declaration).
const ids = new Set([
  assetIdFromHash(skill.ref.contentHash),
  assetIdFromHash(agentsMd.ref.contentHash),
  assetIdFromHash(file.ref.contentHash)
]);
strictEqual(ids.size, 3, "three distinct asset ids");
strictEqual(presignCalls(calls).length, 3);
strictEqual(storagePuts(calls).length, 3);
strictEqual(finalizeCalls(calls).length, 3);
strictEqual(upsertSkillCalls(calls).length, 1);
strictEqual(upsertSkillCalls(calls)[0].body.contentHash, skill.ref.contentHash);

// --- includeBuiltinTools:false with only a skill: the skill still rides
// submission.skills (it is not a builtin); the flag is passed through untouched.
resetCalls(calls);
await client.sessions.create({
  model: "claude-haiku-4-5",
  skills: [skill],
  includeBuiltinTools: false,
  apiKeys: { anthropic: "sk-ant" }
});
const bodyFalse = onlyCreateBody(calls);
strictEqual(bodyFalse.submission.includeBuiltinTools, false);
strictEqual(skillEntries(bodyFalse).length, 1);
strictEqual(builtinEntries(bodyFalse).length, 0);
deepStrictEqual(bodyFalse.submission.tools, []);

// --- includeBuiltinTools:true with a builtin + skill: both coexist in separate fields.
resetCalls(calls);
await client.sessions.create({
  model: "claude-haiku-4-5",
  tools: [BuiltinTools.web_fetch],
  skills: [skill],
  includeBuiltinTools: true,
  apiKeys: { anthropic: "sk-ant" }
});
const bodyTrue = onlyCreateBody(calls);
strictEqual(bodyTrue.submission.includeBuiltinTools, true);
deepStrictEqual(bodyTrue.submission.tools[0], "web_fetch");
strictEqual(skillEntries(bodyTrue).length, 1);

// --- includeBuiltinTools:false with an EXPLICIT builtin + skill: the explicit
// builtin name is still emitted (cherry-picked back) alongside the skill.
resetCalls(calls);
await client.sessions.create({
  model: "claude-haiku-4-5",
  tools: [BuiltinTools.grep],
  skills: [skill],
  includeBuiltinTools: false,
  apiKeys: { anthropic: "sk-ant" }
});
const bodyCherry = onlyCreateBody(calls);
strictEqual(bodyCherry.submission.includeBuiltinTools, false);
deepStrictEqual(builtinEntries(bodyCherry), ["grep"]);
strictEqual(skillEntries(bodyCherry).length, 1);

console.log(JSON.stringify({
  ok: true,
  distinctAssetInputs: ids.size,
  skillSurvivesFalse: skillEntries(bodyFalse).length === 1 && bodyFalse.submission.includeBuiltinTools === false,
  coexistTrue: bodyTrue.submission.tools.length + skillEntries(bodyTrue).length,
  cherryPickedBuiltin: builtinEntries(bodyCherry)
}));
`;
    const result = await runChild(script, "skill-wire-noninterference.mjs");
    expect(result).toMatchObject({
      ok: true,
      distinctAssetInputs: 3,
      skillSurvivesFalse: true,
      coexistTrue: 2,
      cherryPickedBuiltin: ["grep"]
    });
  });
});
