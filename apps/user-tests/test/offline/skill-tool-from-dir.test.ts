/**
 * Blackbox coverage of `Skill.fromDir` filesystem + frontmatter
 * robustness through a clean installed package.
 *
 * `sdk-session-inputs.test.ts` already proves the happy path (a flat SKILL.md
 * dir rides `submission.skills` as a `{ kind:"skill", name }` ref, ordering,
 * reuse dedup, and the name/description reject surface). This
 * file goes DEEPER on the directory reader + the minimal YAML-frontmatter
 * parser: nested bundles, binary files, symlink skipping, byte-determinism of
 * the content hash, CRLF / BOM / quoted / extra-key frontmatter, and the exact
 * 2048-char description boundary.
 *
 * Same pattern as the reference: each case runs in a child process whose cwd is
 * the user-test install tempdir, so `import "@aexhq/sdk"` resolves the packed
 * artifact, and a fake fetch captures the exact wire request (POST
 * /api/sessions plus the presign -> PUT -> finalize asset flow and skill
 * registry upsert) without a live run. Each child logs one small JSON summary
 * that the vitest assertion checks.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

const CHILD_HARNESS = String.raw`
import { deepStrictEqual, match, ok, strictEqual } from "node:assert/strict";
import { mkdirSync, mkdtempSync, symlinkSync, writeFileSync } from "node:fs";
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
  const fetchFake = async (input, init = {}) => {
    const url =
      typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const method = String(init.method ?? "GET").toUpperCase();
    const headers = headersToObject(init.headers);
    const body = await decodeBody(init.body);
    calls.push({ url, method, headers, body });

    if (url.endsWith("/api/assets/presign")) {
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

    if (url.endsWith("/api/assets/finalize")) {
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

    if (url.endsWith("/api/workspace/skills") && method === "POST") {
      return new Response(JSON.stringify({
        resource: {
          kind: "skill",
          resourceId: "wres_" + "1".repeat(32),
          version: 1,
          assetId: body.assetId,
          contentHash: body && typeof body.contentHash === "string" ? body.contentHash : "sha256:" + "a".repeat(64),
          name: body.name,
          description: body && typeof body.description === "string" ? body.description : "",
          sizeBytes: body && typeof body.sizeBytes === "number" ? body.sizeBytes : 0,
          contentType: body.contentType,
          createdAt: new Date(0).toISOString()
        }
      }), { status: 200, headers: { "content-type": "application/json" } });
    }

    return new Response(JSON.stringify({ ok: true }), {
      status: 200,
      headers: { "content-type": "application/json" }
    });
  };
  return { calls, fetch: fetchFake };
}

function presignCalls(calls) {
  return calls.filter((call) => call.url.endsWith("/api/assets/presign"));
}

function finalizeCalls(calls) {
  return calls.filter((call) => call.url.endsWith("/api/assets/finalize"));
}

function storagePuts(calls) {
  return calls.filter((call) => call.url.includes("object-storage.example.test") && call.method === "PUT");
}

function publishSkillCalls(calls) {
  return calls.filter((call) => call.method === "POST" && call.url.endsWith("/api/workspace/skills"));
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


// A fresh, empty scratch dir under the OS temp root. Callers drop a SKILL.md
// (plus any sibling/nested files) then hand the dir to Skill.fromDir.
function freshDir() {
  return mkdtempSync(join(tmpdir(), "aex-skilltool-"));
}
`;

describe("Skill.fromDir filesystem + frontmatter robustness (installed package)", () => {
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

  it("bundles nested dirs + binary files into one deterministic, content-addressed asset", async () => {
    const script = CHILD_HARNESS + String.raw`
const { Aex, Skill } = await importSdk();

function makeClient() {
  const harness = makeFetch();
  const client = new Aex({
    apiKey: "aex_skilltool_token",
    baseUrl: "https://example.invalid",
    fetch: harness.fetch
  });
  return { ...harness, client };
}

// A skill with subdirectories: scripts/helper.js + docs/ref.md alongside SKILL.md.
function writeNestedSkill(overrides = {}) {
  const dir = freshDir();
  writeFileSync(
    join(dir, "SKILL.md"),
    overrides.skillMd ?? "---\nname: nested-skill\ndescription: Bundles nested files.\n---\n# nested-skill\nBody.\n"
  );
  mkdirSync(join(dir, "scripts"), { recursive: true });
  writeFileSync(join(dir, "scripts", "helper.js"), overrides.helper ?? "export const help = () => 42;\n");
  mkdirSync(join(dir, "docs"), { recursive: true });
  writeFileSync(join(dir, "docs", "ref.md"), overrides.ref ?? "# Reference\nSee helper.\n");
  return dir;
}

// 1. Two INDEPENDENT builds of byte-identical nested trees yield an identical
//    content hash / assetId (mtimes are pinned to the zip epoch, entries sorted).
const a = await Skill.fromDir(writeNestedSkill());
const b = await Skill.fromDir(writeNestedSkill());
strictEqual(a.ref.contentHash, b.ref.contentHash, "independent builds must be byte-deterministic");
strictEqual(assetIdFromHash(a.ref.contentHash), assetIdFromHash(b.ref.contentHash));

// 2. The subdirectory files genuinely participate: a bundle WITHOUT them hashes
//    differently, and a one-byte change to a nested file diverges the hash.
const flatDir = freshDir();
writeFileSync(join(flatDir, "SKILL.md"), "---\nname: nested-skill\ndescription: Bundles nested files.\n---\n# nested-skill\nBody.\n");
const flat = await Skill.fromDir(flatDir);
ok(flat.ref.contentHash !== a.ref.contentHash, "nested files must contribute to the bundle hash");
const changed = await Skill.fromDir(writeNestedSkill({ helper: "export const help = () => 43;\n" }));
ok(changed.ref.contentHash !== a.ref.contentHash, "a one-byte nested change must diverge the hash");

// 3. Publishing uploads exactly one asset and returns an immutable workspace ref.
const c = makeClient();
const nested = await Skill.fromDir(writeNestedSkill());
const published = await c.client.workspace.skills.publish(nested);
strictEqual(presignCalls(c.calls).length, 1);
strictEqual(storagePuts(c.calls).length, 1);
strictEqual(finalizeCalls(c.calls).length, 1);
strictEqual(publishSkillCalls(c.calls).length, 1);
ok(storagePuts(c.calls)[0].body && storagePuts(c.calls)[0].body.byteLength > 0, "real zip bytes uploaded");
deepStrictEqual(publishSkillCalls(c.calls)[0].body, {
  assetId: assetIdFromHash(nested.ref.contentHash),
  contentHash: nested.ref.contentHash,
  description: "Bundles nested files.",
  sizeBytes: storagePuts(c.calls)[0].body.byteLength,
  contentType: "application/zip",
  name: "nested-skill"
});
deepStrictEqual(published, {
  kind: "skill",
  resourceId: "wres_" + "1".repeat(32),
  version: 1,
  assetId: assetIdFromHash(nested.ref.contentHash),
  contentHash: nested.ref.contentHash,
  name: "nested-skill",
  description: "Bundles nested files.",
  sizeBytes: storagePuts(c.calls)[0].body.byteLength,
  contentType: "application/zip",
  createdAt: new Date(0).toISOString()
});

// 4. A binary (non-UTF-8) file bundles cleanly and is deterministic across builds.
function writeBinarySkill() {
  const dir = freshDir();
  writeFileSync(join(dir, "SKILL.md"), "---\nname: binary-skill\ndescription: Carries a binary asset.\n---\n# binary-skill\n");
  mkdirSync(join(dir, "assets"), { recursive: true });
  // Bytes that are not valid UTF-8 (NUL, 0xFD-0xFF, high/low mix).
  writeFileSync(join(dir, "assets", "blob.bin"), Uint8Array.from([0, 1, 2, 253, 254, 255, 0, 128, 64, 200]));
  return dir;
}
const bin1 = await Skill.fromDir(writeBinarySkill());
const bin2 = await Skill.fromDir(writeBinarySkill());
strictEqual(bin1.ref.contentHash, bin2.ref.contentHash, "binary bundle must be deterministic");
ok(bin1.ref.contentHash !== a.ref.contentHash);

// 5. Content dedup across DIFFERENT dirs with byte-identical files; a one-byte
//    change to any file diverges the assetId.
function writeDedupSkill(token) {
  const dir = freshDir();
  writeFileSync(join(dir, "SKILL.md"), "---\nname: dedup-skill\ndescription: Deduped by bytes.\n---\n# dedup-skill\n");
  writeFileSync(join(dir, "data.txt"), "payload-" + token + "\n");
  return dir;
}
const dd1 = await Skill.fromDir(writeDedupSkill("same"));
const dd2 = await Skill.fromDir(writeDedupSkill("same"));
const dd3 = await Skill.fromDir(writeDedupSkill("diff"));
strictEqual(dd1.ref.contentHash, dd2.ref.contentHash, "byte-identical dirs must dedup to one assetId");
ok(dd1.ref.contentHash !== dd3.ref.contentHash, "a one-byte change must diverge the assetId");

console.log(JSON.stringify({
  ok: true,
  deterministic: a.ref.contentHash === b.ref.contentHash,
  nestedChangesHash: flat.ref.contentHash !== a.ref.contentHash,
  presign: presignCalls(c.calls).length,
  resourceId: published.resourceId,
  binaryDeterministic: bin1.ref.contentHash === bin2.ref.contentHash,
  dedup: dd1.ref.contentHash === dd2.ref.contentHash,
  diverged: dd1.ref.contentHash !== dd3.ref.contentHash
}));
`;
    const result = await runChild(script, "skill-tool-from-dir-bundle.mjs");
    expect(result).toMatchObject({
      ok: true,
      deterministic: true,
      nestedChangesHash: true,
      presign: 1,
      resourceId: `wres_${"1".repeat(32)}`,
      binaryDeterministic: true,
      dedup: true,
      diverged: true
    });
  });

  it("captures symlinks into the fidelity sidecar when reading the directory", async () => {
    const script = CHILD_HARNESS + String.raw`
const { Skill } = await importSdk();

function writeBaseSkill(dir) {
  writeFileSync(join(dir, "SKILL.md"), "---\nname: symlink-skill\ndescription: Symlinks are captured with fidelity.\n---\n# symlink-skill\n");
  writeFileSync(join(dir, "real.txt"), "real content\n");
}

// Baseline: SKILL.md + one regular file, no symlink.
const baseDir = freshDir();
writeBaseSkill(baseDir);
const base = await Skill.fromDir(baseDir);

// Same regular files PLUS a symlink alias. Phase B2.5 fidelity CAPTURES the
// symlink into the '.aexmeta.json' sidecar (verbatim target — the container
// restore decides whether it may be recreated), so the bundle hash DIVERGES
// from the symlink-free baseline rather than staying byte-identical.
const linkDir = freshDir();
writeBaseSkill(linkDir);
let symlinkSupported = false;
let captured = null;
try {
  // "file" type matters on Windows; harmless elsewhere.
  symlinkSync(join(linkDir, "real.txt"), join(linkDir, "alias.txt"), "file");
  symlinkSupported = true;
} catch {
  // Windows without Developer Mode / elevated rights cannot create symlinks.
  // That is a platform limitation, not a product defect — do not fail here.
  symlinkSupported = false;
}
const linked = await Skill.fromDir(linkDir);
if (symlinkSupported) {
  captured = base.ref.contentHash !== linked.ref.contentHash;
  strictEqual(captured, true, "symlink entry must be CAPTURED into the fidelity sidecar (bundle hash changes)");
}

console.log(JSON.stringify({ ok: true, symlinkSupported, captured }));
`;
    const result = await runChild(script, "skill-tool-from-dir-symlink.mjs");
    expect(result).toMatchObject({ ok: true });
    if (result.symlinkSupported) {
      expect(result.captured).toBe(true);
    }
  });

  it("lifts + overrides name/description across CRLF, BOM, quoted, and extra-key frontmatter", async () => {
    const script = CHILD_HARNESS + String.raw`
const { Aex, Skill } = await importSdk();

function makeClient() {
  const harness = makeFetch();
  const client = new Aex({
    apiKey: "aex_skilltool_token",
    baseUrl: "https://example.invalid",
    fetch: harness.fetch
  });
  return { ...harness, client };
}

function writeSkill(skillMd) {
  const dir = freshDir();
  writeFileSync(join(dir, "SKILL.md"), skillMd);
  return dir;
}

// { name } override takes precedence over the frontmatter name; description
// still comes from the frontmatter.
const overrideSkill = await Skill.fromDir(
  writeSkill("---\nname: frontmatter-name\ndescription: Override wins.\n---\n# x\n"),
  { name: "override-name" }
);
strictEqual(overrideSkill.ref.name, "override-name", "explicit name overrides frontmatter");
strictEqual(overrideSkill.ref.description, "Override wins.");

// Frontmatter parser variants. The bom value's leading character is a real
// U+FEFF byte-order-mark (String.raw preserves it verbatim into the script
// file; the child's JS parser reads it as the first string char), so writing
// it back produces a SKILL.md that genuinely starts with a UTF-8 BOM.
const variants = {
  crlf: "---\r\nname: crlf-skill\r\ndescription: Handles CRLF endings.\r\n---\r\n# body\r\n",
  bom: "﻿---\nname: bom-skill\ndescription: Handles a UTF-8 BOM.\n---\n# body\n",
  single: "---\nname: 'single-skill'\ndescription: 'Single quoted: kept.'\n---\n# body\n",
  double: "---\nname: \"double-skill\"\ndescription: \"Double quoted value.\"\n---\n# body\n",
  extra: "---\nversion: 1.2.3\nname: extra-keys-skill\nlicense: MIT\ndescription: Extra keys ignored.\ntags: a,b,c\n---\n# body\n"
};
const parsed = {};
for (const [key, md] of Object.entries(variants)) {
  const skill = await Skill.fromDir(writeSkill(md));
  parsed[key] = { name: skill.ref.name, description: skill.ref.description };
}
deepStrictEqual(parsed.crlf, { name: "crlf-skill", description: "Handles CRLF endings." });
deepStrictEqual(parsed.bom, { name: "bom-skill", description: "Handles a UTF-8 BOM." });
deepStrictEqual(parsed.single, { name: "single-skill", description: "Single quoted: kept." });
deepStrictEqual(parsed.double, { name: "double-skill", description: "Double quoted value." });
deepStrictEqual(parsed.extra, { name: "extra-keys-skill", description: "Extra keys ignored." });

// Description boundary: EXACTLY 2048 chars is accepted (> 2048 is the reject
// condition, covered by the reject case).
const desc2048 = "d".repeat(2048);
const boundarySkill = await Skill.fromDir(
  writeSkill("---\nname: boundary-skill\ndescription: " + desc2048 + "\n---\n# x\n")
);
strictEqual(boundarySkill.ref.description.length, 2048, "2048-char description accepted");

// Publication confirmation: override + boundary skills preserve lifted fields.
const c = makeClient();
const wire = await Promise.all([
  c.client.workspace.skills.publish(overrideSkill),
  c.client.workspace.skills.publish(boundarySkill)
]);
strictEqual(wire.length, 2);
strictEqual(wire[0].name, "override-name");
strictEqual(wire[1].name, "boundary-skill");
strictEqual(publishSkillCalls(c.calls).length, 2);
const boundaryUpsert = publishSkillCalls(c.calls).find((call) => call.body.name === "boundary-skill");
ok(boundaryUpsert, "boundary skill publication captured");
strictEqual(boundaryUpsert.body.description.length, 2048);

console.log(JSON.stringify({
  ok: true,
  parsed,
  overrideName: wire[0].name,
  boundaryLen: boundaryUpsert.body.description.length
}));
`;
    const result = await runChild(script, "skill-tool-from-dir-frontmatter.mjs");
    expect(result).toMatchObject({
      ok: true,
      parsed: {
        crlf: { name: "crlf-skill", description: "Handles CRLF endings." },
        bom: { name: "bom-skill", description: "Handles a UTF-8 BOM." },
        single: { name: "single-skill", description: "Single quoted: kept." },
        double: { name: "double-skill", description: "Double quoted value." },
        extra: { name: "extra-keys-skill", description: "Extra keys ignored." }
      },
      overrideName: "override-name",
      boundaryLen: 2048
    });
  });

  it("rejects missing frontmatter fields, oversized descriptions, and invalid names", async () => {
    const script = CHILD_HARNESS + String.raw`
const { Skill } = await importSdk();

function writeSkillMd(skillMd) {
  const dir = freshDir();
  writeFileSync(join(dir, "SKILL.md"), skillMd);
  return dir;
}

// No frontmatter block at all — fromDir can derive a name from the temp dir
// basename, so the caller reports the missing description.
const noFrontDir = writeSkillMd("# Just a heading, no frontmatter.\nBody only.\n");
// A dir with no SKILL.md at its root is not a skill bundle.
const noSkillMdDir = freshDir();
writeFileSync(join(noSkillMdDir, "README.md"), "not a skill\n");
// 2049 chars is one over the cap.
const oversizedDir = writeSkillMd("---\nname: over-skill\ndescription: " + "d".repeat(2049) + "\n---\n# x\n");
// A "__" in the frontmatter name is reserved for MCP routing.
const dunderDir = writeSkillMd("---\nname: bad__name\ndescription: has a reserved separator.\n---\n# x\n");
// A valid dir to exercise the { name } override validation surface.
const validDir = writeSkillMd("---\nname: valid-name\ndescription: A valid skill.\n---\n# x\n");

const cases = [
  ["no frontmatter -> missing description", () => Skill.fromDir(noFrontDir), /description is required/],
  ["no frontmatter + override -> missing description", () => Skill.fromDir(noFrontDir, { name: "override-name" }), /description is required/],
  ["missing SKILL.md", () => Skill.fromDir(noSkillMdDir), /must contain a SKILL\.md/],
  ["oversized description", () => Skill.fromDir(oversizedDir), /description must be <= 2048 chars/],
  ["reserved __ in frontmatter name", () => Skill.fromDir(dunderDir), /must not contain "__"/]
];

const messages = [];
for (const [label, fn, pattern] of cases) {
  messages.push(await expectReject(label, fn, pattern));
}

// Name fuzz via { name } override on a valid dir: uppercase, spaces, leading
// "-", reserved "__", >128 chars, path separators, and empty are all rejected.
const invalidNames = ["", "UPPER", "two words", "-starts-bad", "bad__name", "a".repeat(129), "slash/name", "dot.name"];
for (const name of invalidNames) {
  await expectReject(
    "name fuzz " + JSON.stringify(name),
    () => Skill.fromDir(validDir, { name }),
    /must match|name is required|must not contain "__"/
  );
}

// A valid kebab name overriding the frontmatter is accepted.
const okSkill = await Skill.fromDir(validDir, { name: "valid-kebab-1" });
strictEqual(okSkill.ref.name, "valid-kebab-1");
strictEqual(okSkill.isDraft, true);

console.log(JSON.stringify({
  ok: true,
  rejects: cases.length,
  fuzzRejects: invalidNames.length,
  accepted: okSkill.ref.name,
  sample: messages[0]
}));
`;
    const result = await runChild(script, "skill-tool-from-dir-reject.mjs");
    expect(result).toMatchObject({
      ok: true,
      rejects: 5,
      fuzzRejects: 8,
      accepted: "valid-kebab-1"
    });
  });
});
