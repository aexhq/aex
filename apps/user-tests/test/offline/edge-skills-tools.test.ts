/**
 * Offline edge-case sweep for the SKILLS & TOOLS composition surface, exercised
 * through a clean installed `@aexhq/sdk` (blackbox, child process, cwd = install
 * tempdir so `import "@aexhq/sdk"` resolves the packed artifact). No live run —
 * every case is SDK-side validation / wire-shape.
 *
 * These target gaps NOT already covered by the sibling offline suites
 * (`skill-tool-from-dir` / `skill-tool-from-url` / `skill-tool-wire-and-dedup`):
 *
 *   A. Skill-bundle path safety + hard limits via `bundleSkillFiles` directly —
 *      path traversal (`..`), absolute/drive-letter paths, backslash separators,
 *      NUL bytes, trailing slash, depth > 16, path length > 512, empty map,
 *      missing SKILL.md, > 1000 files, an oversized (> 50 MB) file.
 *   B. `Tool.fromFiles` manifest validation — missing/empty/array input schema,
 *      non-"object" schema type, empty / oversized description, reserved "__" in
 *      the name, entry missing from files, reserved "tool.json" key, empty files
 *      map, and path traversal in the entry or a file key. Plus the accept path
 *      (snake_case `input_schema` alias).
 *   C. Builtin selection + custom-tool wire semantics via a fake-fetch client —
 *      `tools: []` serialises to `[]`; `includeBuiltinTools:false` +
 *      `[BuiltinTools.bash]` cherry-picks exactly `["bash"]`; an unknown builtin
 *      name string is rejected before any HTTP; duplicate builtin names dedup;
 *      two DISTINCT custom Tools with the SAME name are NOT deduped client-side
 *      (both ride the wire — the collision is the server's to resolve).
 *
 * Limits are the `SKILL_BUNDLE_LIMITS` contract values (maxFiles 1000,
 * maxDecompressedBytes 50 MiB, maxDepth 16, maxPathLength 512), hardcoded here
 * with a reference since the SDK does not re-export the limits object.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

const CHILD_HARNESS = String.raw`
import { deepStrictEqual, match, ok, strictEqual } from "node:assert/strict";

async function expectReject(label, fn, pattern) {
  try {
    await fn();
  } catch (err) {
    const message = err && err.message ? err.message : String(err);
    if (pattern) match(message, pattern, label + " :: " + message);
    return message;
  }
  throw new Error(label + " unexpectedly resolved");
}

function headersToObject(headers) {
  const out = {};
  if (!headers) return out;
  if (headers instanceof Headers) {
    for (const [k, v] of headers.entries()) out[k.toLowerCase()] = v;
    return out;
  }
  if (Array.isArray(headers)) {
    for (const [k, v] of headers) out[String(k).toLowerCase()] = String(v);
    return out;
  }
  for (const [k, v] of Object.entries(headers)) out[String(k).toLowerCase()] = String(v);
  return out;
}

async function decodeBody(body) {
  if (body === undefined || body === null) return undefined;
  if (typeof body === "string") {
    try { return JSON.parse(body); } catch { return body; }
  }
  if (body instanceof Uint8Array) return { kind: "Uint8Array", byteLength: body.byteLength };
  if (body instanceof ArrayBuffer) return { kind: "ArrayBuffer", byteLength: body.byteLength };
  try {
    const text = await new Response(body).text();
    try { return JSON.parse(text); } catch { return text; }
  } catch { return String(body); }
}

// Content-addressed fake asset store + /api/sessions, mirroring the sibling
// offline harnesses. A hash already PUT presigns as a dedup hit.
function makeFetch() {
  const calls = [];
  const stored = new Set();
  let sessionCounter = 0;
  const fetchFake = async (input, init = {}) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const method = String(init.method ?? "GET").toUpperCase();
    const headers = headersToObject(init.headers);
    const body = await decodeBody(init.body);
    calls.push({ url, method, headers, body });
    const j = (status, obj) => new Response(JSON.stringify(obj), { status, headers: { "content-type": "application/json" } });

    if (url.endsWith("/assets/presign")) {
      const hash = body && typeof body.hash === "string" ? body.hash : "sha256:" + "a".repeat(64);
      const hex = hash.startsWith("sha256:") ? hash.slice(7) : hash;
      if (stored.has(hash)) return j(200, { ok: true, exists: true, assetId: "asset_" + hex, contentHash: hash, sizeBytes: 0 });
      return j(201, {
        ok: true, exists: false, assetId: "asset_" + hex, contentHash: hash,
        uploadUrl: "https://object-storage.example.test/assets/" + hex + "?sig=test",
        requiredHeaders: { "x-amz-checksum-sha256": "Y2hlY2tzdW0=" }, expiresInSeconds: 300
      });
    }
    if (url.includes("object-storage.example.test")) {
      const m = /\/assets\/([0-9a-f]{64})/.exec(url);
      if (m) stored.add("sha256:" + m[1]);
      return new Response("", { status: 200 });
    }
    if (url.endsWith("/assets/finalize")) {
      const hash = body && typeof body.hash === "string" ? body.hash : "sha256:" + "a".repeat(64);
      const hex = hash.startsWith("sha256:") ? hash.slice(7) : hash;
      return j(200, { ok: true, exists: false, assetId: "asset_" + hex, contentHash: hash, sizeBytes: 0 });
    }
    if (url.endsWith("/api/sessions") && method === "POST") {
      sessionCounter += 1;
      return j(201, { session: { id: "sess_edge_" + sessionCounter, workspaceId: "ws_edge", status: "idle", turnSeq: 0, createdAt: new Date(0).toISOString() } });
    }
    return j(200, { ok: true });
  };
  return { calls, fetch: fetchFake };
}

function onlyCreateBody(calls) {
  const bodies = calls.filter((c) => c.method === "POST" && c.url.endsWith("/api/sessions")).map((c) => c.body);
  strictEqual(bodies.length, 1, "expected exactly one session-create call");
  return bodies[0];
}
`;

// SKILL_BUNDLE_LIMITS (contract), referenced so the intent is explicit.
const LIMIT_MAX_FILES = 1000;
const LIMIT_MAX_DEPTH = 16;
const LIMIT_MAX_PATH = 512;
const LIMIT_MAX_DECOMPRESSED = 50 * 1024 * 1024;

describe("edge: skills & tools composition (offline, installed package)", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex();
  }, 240_000);

  afterAll(() => {
    install?.cleanup();
  });

  async function runChild(script: string, fileName: string, timeoutMs = 120_000): Promise<Record<string, unknown>> {
    const scriptPath = join(install.installDir, fileName);
    writeFileSync(scriptPath, script);
    const child = await runCommand(getBunCommand(), [scriptPath], { cwd: install.installDir, timeoutMs });
    if (child.exitCode !== 0) {
      throw new Error(`${fileName} exited ${child.exitCode}\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`);
    }
    return JSON.parse(child.stdout.trim()) as Record<string, unknown>;
  }

  it("A: bundleSkillFiles rejects unsafe paths and enforces bundle limits", async () => {
    const script =
      CHILD_HARNESS +
      String.raw`
const { bundleSkillFiles } = await import("@aexhq/sdk");
const SKILL = "---\nname: edge-skill\ndescription: Edge probe.\n---\n# edge\n";
const NUL = String.fromCharCode(0);

// Every case pairs a valid SKILL.md with ONE offending sibling entry, so the
// only reason to throw is the offending path/limit.
const bad = [
  ["parent traversal", { "SKILL.md": SKILL, "../evil.txt": "x" }, /'\.\.' segment/],
  ["nested traversal", { "SKILL.md": SKILL, "a/../../evil.txt": "x" }, /'\.\.' segment/],
  ["absolute path", { "SKILL.md": SKILL, "/etc/passwd": "x" }, /must be relative/],
  ["drive letter", { "SKILL.md": SKILL, "C:/windows/x.txt": "x" }, /drive letter/],
  ["backslash sep", { "SKILL.md": SKILL, "a\\b.txt": "x" }, /backslash separator/],
  ["NUL byte", { "SKILL.md": SKILL, ["a" + NUL + "b.txt"]: "x" }, /NUL byte/],
  ["trailing slash", { "SKILL.md": SKILL, "dir/": "x" }, /must not end with '\/'/],
  ["dot segment", { "SKILL.md": SKILL, "./x.txt": "x" }, /empty or '\.' segment/],
  ["empty key", { "SKILL.md": SKILL, "": "x" }, /must be non-empty/],
  ["too deep", { "SKILL.md": SKILL, [Array.from({ length: ${LIMIT_MAX_DEPTH} + 1 }, (_, i) => "d" + i).join("/") + "/f.txt"]: "x" }, /maxDepth/],
  ["path too long", { "SKILL.md": SKILL, ["a/" + "z".repeat(${LIMIT_MAX_PATH})]: "x" }, /maxPathLength/]
];
const msgs = {};
for (const [label, files, pat] of bad) {
  msgs[label] = await expectReject(label, async () => bundleSkillFiles(files), pat);
}

// Empty map + missing SKILL.md are their own rejects.
await expectReject("empty map", async () => bundleSkillFiles({}), /cannot be empty/);
await expectReject("no SKILL.md", async () => bundleSkillFiles({ "notes.txt": "hi" }), /must contain a "SKILL\.md"/);

// > maxFiles regular entries.
const many = { "SKILL.md": SKILL };
for (let i = 0; i < ${LIMIT_MAX_FILES} + 5; i++) many["f" + i + ".txt"] = "x";
const manyMsg = await expectReject("too many files", async () => bundleSkillFiles(many), /file limit|maxFiles/);

// A single oversized (> 50 MiB decompressed) file. The per-entry size check
// fires before zipping, so this never allocates a giant zip.
const bigMsg = await expectReject(
  "oversized file",
  async () => bundleSkillFiles({ "SKILL.md": SKILL, "big.bin": new Uint8Array(${LIMIT_MAX_DECOMPRESSED} + 1) }),
  /maxDecompressedBytes|decompressed cap/
);

// Sanity: a clean bundle still builds.
const good = bundleSkillFiles({ "SKILL.md": SKILL, "data/ok.txt": "fine\n" });
ok(good.zip instanceof Uint8Array && good.zip.byteLength > 0, "clean bundle builds");

console.log(JSON.stringify({
  ok: true,
  rejected: Object.keys(msgs).length,
  manyRejected: /file limit|maxFiles/.test(manyMsg),
  bigRejected: /maxDecompressedBytes|decompressed cap/.test(bigMsg),
  sample: msgs["parent traversal"]
}));
`;
    const result = await runChild(script, "edge-bundle-paths.mjs");
    expect(result).toMatchObject({ ok: true, rejected: 11, manyRejected: true, bigRejected: true });
  });

  it("B: Tool.fromFiles validates manifest, schema, name, entry, and file paths", async () => {
    const script =
      CHILD_HARNESS +
      String.raw`
const { Tool } = await import("@aexhq/sdk");
const okSchema = { type: "object", properties: { q: { type: "string" } }, required: ["q"] };
const base = { name: "ok_tool", description: "A fine tool.", entry: "index.js", files: { "index.js": "export default async () => 'x';\n" } };

const cases = [
  ["missing inputSchema", { ...base }, /inputSchema must be a JSON Schema object/],
  ["null inputSchema", { ...base, inputSchema: null }, /inputSchema must be a JSON Schema object/],
  ["array inputSchema", { ...base, inputSchema: [] }, /inputSchema must be a JSON Schema object/],
  ["schema type not object", { ...base, inputSchema: { type: "string" } }, /inputSchema\.type must be "object"/],
  ["empty description", { ...base, inputSchema: okSchema, description: "   " }, /description must be non-empty/],
  ["oversized description", { ...base, inputSchema: okSchema, description: "d".repeat(2049) }, /description must be non-empty and <= 2048/],
  ["reserved __ in name", { ...base, inputSchema: okSchema, name: "bad__name" }, /must not contain "__"/],
  ["invalid name UPPER", { ...base, inputSchema: okSchema, name: "BadName" }, /name must match/],
  ["invalid name spaces", { ...base, inputSchema: okSchema, name: "two words" }, /name must match/],
  ["entry not in files", { ...base, inputSchema: okSchema, entry: "missing.js" }, /entry "missing\.js" must exist/],
  ["reserved tool.json key", { ...base, inputSchema: okSchema, files: { "index.js": "export default async () => 'x';\n", "tool.json": "{}" } }, /must not include reserved "tool\.json"/],
  ["empty files map", { ...base, inputSchema: okSchema, files: {} }, /files map cannot be empty/],
  ["path traversal in entry", { ...base, inputSchema: okSchema, entry: "../evil.js" }, /'\.\.' segment/],
  ["path traversal file key", { ...base, inputSchema: okSchema, files: { "index.js": "export default async () => 'x';\n", "../evil.js": "x" } }, /'\.\.' segment/]
];
const msgs = {};
for (const [label, args, pat] of cases) {
  msgs[label] = await expectReject(label, async () => Tool.fromFiles(args), pat);
}

// Accept path: snake_case input_schema alias is honoured; a valid tool builds a draft.
const snake = await Tool.fromFiles({ name: "snake_tool", description: "Uses input_schema.", input_schema: okSchema, entry: "index.js", files: { "index.js": "export default async () => 'x';\n" } });
strictEqual(snake.isDraft, true);
strictEqual(snake.ref.kind, "draft");
strictEqual(snake.ref.name, "snake_tool");
deepStrictEqual(snake.ref.input_schema, okSchema);

console.log(JSON.stringify({ ok: true, rejected: Object.keys(msgs).length, snakeName: snake.ref.name, sample: msgs["missing inputSchema"] }));
`;
    const result = await runChild(script, "edge-tool-validate.mjs");
    expect(result).toMatchObject({ ok: true, rejected: 14, snakeName: "snake_tool" });
  });

  it("C: builtin selection + custom-tool name-collision wire semantics", async () => {
    const script =
      CHILD_HARNESS +
      String.raw`
const { Aex, BuiltinTools, Tool } = await import("@aexhq/sdk");
const okSchema = { type: "object", properties: {} };

function makeClient() {
  const { calls, fetch } = makeFetch();
  return { calls, client: new Aex({ apiKey: "aex_edge_token", baseUrl: "https://example.invalid", fetch }) };
}

// 1. tools: [] serialises to an empty tools array (no throw, no builtins injected client-side).
{
  const c = makeClient();
  await c.client.sessions.create({ model: "claude-haiku-4-5", tools: [], apiKeys: { anthropic: "sk-ant" } });
  deepStrictEqual(onlyCreateBody(c.calls).submission.tools, []);
}

// 2. includeBuiltinTools:false + a single cherry-picked builtin => exactly ["bash"].
let cherry;
{
  const c = makeClient();
  await c.client.sessions.create({ model: "claude-haiku-4-5", includeBuiltinTools: false, tools: [BuiltinTools.bash], apiKeys: { anthropic: "sk-ant" } });
  const body = onlyCreateBody(c.calls);
  strictEqual(body.submission.includeBuiltinTools, false);
  deepStrictEqual(body.submission.tools, ["bash"]);
  cherry = body.submission.tools;
}

// 3. Duplicate builtin names dedup to one, in first-seen order.
{
  const c = makeClient();
  await c.client.sessions.create({ model: "claude-haiku-4-5", tools: [BuiltinTools.bash, BuiltinTools.bash, BuiltinTools.grep, BuiltinTools.bash], apiKeys: { anthropic: "sk-ant" } });
  deepStrictEqual(onlyCreateBody(c.calls).submission.tools, ["bash", "grep"]);
}

// 4. An unknown builtin name string is rejected BEFORE any HTTP.
let unknownMsg;
{
  const c = makeClient();
  unknownMsg = await expectReject(
    "unknown builtin",
    async () => c.client.sessions.create({ model: "claude-haiku-4-5", tools: ["definitely_not_a_builtin"], apiKeys: { anthropic: "sk-ant" } }),
    /is not a builtin tool name/
  );
  strictEqual(c.calls.length, 0, "no HTTP on invalid tool name");
}

// 5. Two DISTINCT custom Tools with the SAME name + DIFFERENT bytes are NOT
//    deduped client-side: both ride the wire as separate asset entries. The SDK
//    does not resolve the collision — it reaches the server (backs the live
//    duplicate-name probe).
let dupWire;
{
  const c = makeClient();
  const tA = await Tool.fromFiles({ name: "dup_tool", description: "Alpha.", inputSchema: okSchema, entry: "index.js", files: { "index.js": "export default async () => 'A';\n" } });
  const tB = await Tool.fromFiles({ name: "dup_tool", description: "Bravo.", inputSchema: okSchema, entry: "index.js", files: { "index.js": "export default async () => 'B';\n" } });
  ok(tA.ref.contentHash !== tB.ref.contentHash, "different bytes hash differently");
  await c.client.sessions.create({ model: "claude-haiku-4-5", includeBuiltinTools: false, tools: [tA, tB], apiKeys: { anthropic: "sk-ant" } });
  const entries = onlyCreateBody(c.calls).submission.tools.filter((e) => e && typeof e === "object" && e.kind === "asset");
  strictEqual(entries.length, 2, "both same-named custom tools ride the wire");
  strictEqual(entries[0].name, "dup_tool");
  strictEqual(entries[1].name, "dup_tool");
  ok(entries[0].assetId !== entries[1].assetId, "distinct assets despite identical name");
  dupWire = { count: entries.length, distinctAssets: entries[0].assetId !== entries[1].assetId };
}

console.log(JSON.stringify({ ok: true, cherry, unknownRejected: /is not a builtin tool name/.test(unknownMsg), dupWire }));
`;
    const result = await runChild(script, "edge-builtin-wire.mjs");
    expect(result).toMatchObject({
      ok: true,
      cherry: ["bash"],
      unknownRejected: true,
      dupWire: { count: 2, distinctAssets: true }
    });
  });
});
