/**
 * Offline edge-case sweep for the AGENTS.MD + FILES/ASSETS composition surface,
 * exercised through a clean installed `@aexhq/sdk` (blackbox, child process,
 * cwd = install tempdir so `import "@aexhq/sdk"` resolves the packed artifact).
 * No live run — every case is SDK-side validation / wire-shape / dedup.
 *
 * These target the client-side contract of the two primitives whose LIVE
 * behaviour is covered by the sibling `test/live/edge-agentsmd-files.user.test.ts`:
 *
 *   A. `AgentsMd.fromContent(...)` input validation + draft build: empty content,
 *      malformed `name` (uppercase / single-char / leading-or-trailing dash /
 *      too long), large (100 KB) + unicode/markdown content accepted, dedup hash
 *      is a pure function of (content) under a fixed name, and a draft cannot be
 *      JSON-serialised.
 *   B. `File.fromBytes(...)` input validation + path-traversal DEFENCE: a
 *      `../../etc/passwd` (and backslash / `.` / `..` / slash / NUL / empty /
 *      over-255) filename is REJECTED client-side; zero-byte + non-Uint8Array
 *      bytes rejected; malformed `mountPath` (traversal / relative / NUL /
 *      backslash) rejected; filename-with-spaces + unicode filename ACCEPTED
 *      (slugged); `mountPath:"/etc"` is ACCEPTED at the SDK boundary (the runtime
 *      is documented to rebase it under the workspace — the live suite checks
 *      that it actually clamps); content-hash dedup keys on (bytes + filename).
 *   C. Wire composition + content-addressed dedup via a fake-fetch client: an
 *      `agentsMd` + `files` submission serialises both as `kind:"asset"` refs;
 *      the same File instance passed twice, and two DISTINCT instances with
 *      identical bytes, both resolve to ONE `assetId` (store dedup) — no error,
 *      no duplicate upload.
 *
 * The path-traversal filename is the security-relevant case: it is REJECTED at
 * `File.fromBytes`, so it never reaches the wire — asserted here as the good
 * (defended) behaviour.
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
// offline harnesses. A hash already PUT presigns as a dedup hit (exists:true).
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

describe("edge: agents.md + files composition (offline, installed package)", () => {
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

  it("A: AgentsMd.fromContent validates name/content, builds drafts, dedups by content", async () => {
    const script =
      CHILD_HARNESS +
      String.raw`
const { AgentsMd } = await import("@aexhq/sdk");

const bad = [
  ["empty content", () => AgentsMd.fromContent("", { name: "rules" }), /non-empty string/],
  ["whitespace-only name miss", () => AgentsMd.fromContent("# x", { name: "  " }), /name must match/],
  ["name uppercase", () => AgentsMd.fromContent("# x", { name: "Rules" }), /name must match/],
  ["name single char", () => AgentsMd.fromContent("# x", { name: "a" }), /name must match/],
  ["name leading dash", () => AgentsMd.fromContent("# x", { name: "-rules" }), /name must match/],
  ["name trailing dash", () => AgentsMd.fromContent("# x", { name: "rules-" }), /name must match/],
  ["name too long", () => AgentsMd.fromContent("# x", { name: "a".repeat(65) }), /name must match/],
  ["name underscore", () => AgentsMd.fromContent("# x", { name: "my_rules" }), /name must match/]
];
const msgs = {};
for (const [label, fn, pat] of bad) msgs[label] = await expectReject(label, fn, pat);

// Accept path: a valid draft.
const good = await AgentsMd.fromContent("# Be terse\nAlways answer in one word.", { name: "rules" });
strictEqual(good.isDraft, true, "valid agentsMd is a draft");
strictEqual(good.ref.kind, "draft");
strictEqual(good.ref.name, "rules");
ok(typeof good.ref.contentHash === "string" && good.ref.contentHash.length > 0, "draft carries a contentHash");

// Large 100 KB content builds without throwing.
const big = await AgentsMd.fromContent("# Notes\n" + "x".repeat(100 * 1024), { name: "big-rules" });
ok(big.ref.contentHash.length > 0, "100KB agents.md builds");

// Unicode + markdown content builds without throwing.
const uni = await AgentsMd.fromContent("# Cafe ☕ π\n- **bold** _em_ text\n日本語 🦘", { name: "uni-rules" });
ok(uni.ref.contentHash.length > 0, "unicode agents.md builds");

// Dedup: identical content under a fixed name => identical hash; different content => different hash.
const d1 = await AgentsMd.fromContent("# same body", { name: "rr" });
const d2 = await AgentsMd.fromContent("# same body", { name: "rr" });
strictEqual(d1.ref.contentHash, d2.ref.contentHash, "same content => same hash (dedup)");
const d3 = await AgentsMd.fromContent("# different body", { name: "rr" });
ok(d1.ref.contentHash !== d3.ref.contentHash, "different content => different hash");

// A draft cannot be JSON-serialised (only becomes a wire ref after upload).
await expectReject("draft toJSON", async () => good.toJSON(), /cannot be JSON-serialised/);

console.log(JSON.stringify({ ok: true, rejected: Object.keys(msgs).length, goodName: good.ref.name, sample: msgs["name uppercase"] }));
`;
    const result = await runChild(script, "edge-agentsmd-validate.mjs");
    expect(result).toMatchObject({ ok: true, rejected: 8, goodName: "rules" });
  });

  it("B: File.fromBytes defends path traversal, validates bytes/mountPath, builds + dedups", async () => {
    const script =
      CHILD_HARNESS +
      String.raw`
const { File } = await import("@aexhq/sdk");
const B = (s) => new TextEncoder().encode(s);
const NUL = String.fromCharCode(0);

// --- Path-traversal + malformed filename DEFENCE (rejected client-side, never reaches the wire) ---
const badName = [
  ["traversal slash", { name: "../../etc/passwd", bytes: B("x") }, /is not a valid filename/],
  ["traversal backslash", { name: "..\\..\\windows\\system32\\drivers", bytes: B("x") }, /is not a valid filename/],
  ["dotdot", { name: "..", bytes: B("x") }, /is not a valid filename/],
  ["dot", { name: ".", bytes: B("x") }, /is not a valid filename/],
  ["contains slash", { name: "a/b.txt", bytes: B("x") }, /is not a valid filename/],
  ["NUL in name", { name: "a" + NUL + "b.txt", bytes: B("x") }, /is not a valid filename/],
  ["empty name", { name: "", bytes: B("x") }, /is not a valid filename/],
  ["over 255 chars", { name: "a".repeat(256) + ".txt", bytes: B("x") }, /is not a valid filename/],
  ["name not a string", { name: 123, bytes: B("x") }, /name must be a string/]
];
// --- Byte + mountPath validation ---
const badArgs = [
  ["zero bytes", { name: "empty.txt", bytes: new Uint8Array(0) }, /non-empty Uint8Array/],
  ["bytes not Uint8Array", { name: "x.txt", bytes: "not bytes" }, /non-empty Uint8Array/],
  ["mountPath traversal", { name: "x.txt", bytes: B("x"), mountPath: "/workspace/../../etc" }, /traversal/],
  ["mountPath relative", { name: "x.txt", bytes: B("x"), mountPath: "relative/dir" }, /absolute path/],
  ["mountPath backslash", { name: "x.txt", bytes: B("x"), mountPath: "/a\\b" }, /NUL or backslash/],
  ["mountPath NUL", { name: "x.txt", bytes: B("x"), mountPath: "/a" + NUL + "b" }, /NUL or backslash/]
];
const msgs = {};
for (const [label, args, pat] of [...badName, ...badArgs]) {
  msgs[label] = await expectReject(label, async () => File.fromBytes(args), pat);
}

// --- Accept path: tricky-but-legal filenames are slugged, never rejected ---
const spaced = await File.fromBytes({ name: "my report.txt", bytes: B("hi") });
strictEqual(spaced.ref.kind, "draft");
strictEqual(spaced.ref.name, "my-report", "space filename slugs to my-report");
strictEqual(spaced.ref.mountPath, "/workspace", "default mountPath is /workspace");

const uni = await File.fromBytes({ name: "café.txt", bytes: B("hi") });
ok(uni.ref.name.length >= 2, "unicode filename yields a usable slug: " + uni.ref.name);

// mountPath "/etc" is ACCEPTED at the SDK boundary (documented runtime rebase).
const etc = await File.fromBytes({ name: "probe.txt", bytes: B("hi"), mountPath: "/etc" });
strictEqual(etc.ref.mountPath, "/etc", "/etc mountPath passes SDK validation (runtime clamps)");

// --- Dedup: content hash keys on (bytes + filename) ---
const f1 = await File.fromBytes({ name: "data.bin", bytes: B("payload") });
const f2 = await File.fromBytes({ name: "data.bin", bytes: B("payload") });
strictEqual(f1.ref.contentHash, f2.ref.contentHash, "same bytes+name => same hash (dedup)");
const f3 = await File.fromBytes({ name: "data.bin", bytes: B("payload-2") });
ok(f1.ref.contentHash !== f3.ref.contentHash, "different bytes => different hash");
const f4 = await File.fromBytes({ name: "other.bin", bytes: B("payload") });
ok(f1.ref.contentHash !== f4.ref.contentHash, "filename participates in the content hash");

await expectReject("draft toJSON", async () => f1.toJSON(), /cannot be JSON-serialised/);

console.log(JSON.stringify({
  ok: true,
  rejected: Object.keys(msgs).length,
  spacedName: spaced.ref.name,
  uniName: uni.ref.name,
  etcMount: etc.ref.mountPath,
  traversalRejected: /is not a valid filename/.test(msgs["traversal slash"])
}));
`;
    const result = await runChild(script, "edge-file-validate.mjs");
    expect(result).toMatchObject({
      ok: true,
      rejected: 15,
      spacedName: "my-report",
      etcMount: "/etc",
      traversalRejected: true
    });
  });

  it("C: agents.md + files serialise as asset refs and dedup to one assetId", async () => {
    const script =
      CHILD_HARNESS +
      String.raw`
const { Aex, AgentsMd, File } = await import("@aexhq/sdk");
const B = (s) => new TextEncoder().encode(s);

function makeClient() {
  const { calls, fetch } = makeFetch();
  return { calls, client: new Aex({ apiToken: "aex_edge_token", baseUrl: "https://example.invalid", fetch }) };
}

// 1. agents.md + the SAME File instance passed twice => both wire entries share one assetId.
let sameInstance;
{
  const c = makeClient();
  const a = await AgentsMd.fromContent("# rules\nBe terse.", { name: "rules" });
  const f = await File.fromBytes({ name: "data.txt", bytes: B("hello world"), mountPath: "/workspace/data" });
  await c.client.sessions.create({ model: "claude-haiku-4-5", agentsMd: [a], files: [f, f], apiKeys: { anthropic: "sk-ant" } });
  const body = onlyCreateBody(c.calls);
  strictEqual(body.submission.agentsMd.length, 1, "one agents.md ref");
  strictEqual(body.submission.agentsMd[0].kind, "asset");
  strictEqual(body.submission.agentsMd[0].name, "rules");
  strictEqual(body.submission.files.length, 2, "both file entries ride the wire");
  strictEqual(body.submission.files[0].kind, "asset");
  strictEqual(body.submission.files[0].mountPath, "/workspace/data", "custom mountPath preserved on the wire");
  strictEqual(body.submission.files[0].assetId, body.submission.files[1].assetId, "same instance => one assetId");
  sameInstance = { files: body.submission.files.length, dedup: body.submission.files[0].assetId === body.submission.files[1].assetId };
}

// 2. Two DISTINCT File instances with IDENTICAL bytes+name => content-addressed store
//    dedups them to ONE assetId (presign exists:true on the second), no error.
let distinctInstances;
{
  const c = makeClient();
  const g1 = await File.fromBytes({ name: "same.txt", bytes: B("duplicate-bytes") });
  const g2 = await File.fromBytes({ name: "same.txt", bytes: B("duplicate-bytes") });
  ok(g1.ref.contentHash === g2.ref.contentHash, "identical content hashes");
  ok(g1 !== g2, "distinct instances");
  await c.client.sessions.create({ model: "claude-haiku-4-5", files: [g1, g2], apiKeys: { anthropic: "sk-ant" } });
  const body = onlyCreateBody(c.calls);
  strictEqual(body.submission.files[0].assetId, body.submission.files[1].assetId, "distinct instances, same bytes => one assetId (store dedup)");
  // Exactly one PUT to object storage (the second was a presign dedup hit).
  const puts = c.calls.filter((x) => x.method === "PUT" && x.url.includes("object-storage.example.test"));
  strictEqual(puts.length, 1, "identical bytes uploaded once, not twice");
  distinctInstances = { dedup: body.submission.files[0].assetId === body.submission.files[1].assetId, puts: puts.length };
}

console.log(JSON.stringify({ ok: true, sameInstance, distinctInstances }));
`;
    const result = await runChild(script, "edge-file-wire.mjs");
    expect(result).toMatchObject({
      ok: true,
      sameInstance: { files: 2, dedup: true },
      distinctInstances: { dedup: true, puts: 1 }
    });
  });
});
