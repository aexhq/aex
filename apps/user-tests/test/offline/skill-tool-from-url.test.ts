/**
 * Blackbox coverage for `Tools.fromSkillUrl` (URL/zip skill ingestion) through
 * a clean installed `@aexhq/sdk`.
 *
 * `fromSkillUrl` fetches a zip archive (caller-controlled fetch), optionally
 * integrity-checks it against `sha256`, unzips, strips a single top-level
 * folder, requires a root `SKILL.md`, and reduces to the SAME canonical files
 * map as `Tools.fromSkillDir` — so a URL-sourced skill and the identical local
 * dir produce the SAME asset and dedup against each other.
 *
 * These cases run in child processes whose cwd is the user-test install
 * tempdir, so `import "@aexhq/sdk"` resolves from the packed/published
 * artifact. A fake `fetch` serves BOTH the synthetic skill archive (built in
 * the child from `bundleSkillFiles`) and the asset upload + `/api/sessions`
 * wire calls, so the whole ingest → upload → serialize path is exercised
 * without a live run. The child prints one compact JSON summary that the parent
 * asserts on.
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
  if (body instanceof Uint8Array) return { kind: "Uint8Array", byteLength: body.byteLength };
  if (body instanceof ArrayBuffer) return { kind: "ArrayBuffer", byteLength: body.byteLength };
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

// A fake fetch that ALSO serves synthetic skill archives. \`archives\` maps a URL
// to { status, bytes }; anything else falls through to the content-addressable
// asset store (presign -> PUT -> finalize) + /api/sessions handlers. The presign
// handler tracks seen content hashes and returns exists:true on repeat, mirroring
// the real API's content-address dedup (a second identical upload is a no-op PUT).
function makeFetch(archives = {}) {
  const calls = [];
  const seenHashes = new Set();
  let sessionCounter = 0;
  const fetchFake = async (input, init = {}) => {
    const url =
      typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const method = String(init.method ?? "GET").toUpperCase();
    const headers = headersToObject(init.headers);
    const body = await decodeBody(init.body);
    calls.push({ url, method, headers, body });

    // Synthetic skill archive download.
    if (Object.prototype.hasOwnProperty.call(archives, url)) {
      const route = archives[url];
      const status = route.status ?? 200;
      if (status < 200 || status >= 300) {
        return new Response("", { status });
      }
      return new Response(route.bytes ?? new Uint8Array(), { status });
    }

    if (url.endsWith("/assets/presign")) {
      const hash = body && typeof body.hash === "string" ? body.hash : "sha256:" + "a".repeat(64);
      const hex = hash.startsWith("sha256:") ? hash.slice("sha256:".length) : hash;
      if (seenHashes.has(hex)) {
        // Content-address dedup hit: identical bytes already vaulted -> no PUT.
        return new Response(JSON.stringify({
          ok: true,
          exists: true,
          assetId: "asset_" + hex,
          contentHash: hash,
          sizeBytes: body && typeof body.sizeBytes === "number" ? body.sizeBytes : 0
        }), { status: 200, headers: { "content-type": "application/json" } });
      }
      seenHashes.add(hex);
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

    if (url.endsWith("/api/sessions") && method === "POST") {
      sessionCounter += 1;
      return new Response(JSON.stringify({
        session: {
          id: "sess_skill_url_" + sessionCounter,
          workspaceId: "ws_skill_url",
          status: "idle",
          turnSeq: 0,
          createdAt: new Date(0).toISOString()
        }
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

function presignCalls(calls) {
  return calls.filter((call) => call.url.endsWith("/assets/presign"));
}

function finalizeCalls(calls) {
  return calls.filter((call) => call.url.endsWith("/assets/finalize"));
}

function storagePuts(calls) {
  return calls.filter((call) => call.url.includes("object-storage.example.test") && call.method === "PUT");
}

function archiveFetches(calls, host) {
  return calls.filter((call) => call.url.includes(host) && call.method === "GET");
}

function skillToolEntries(body) {
  return body.submission.tools.filter(
    (entry) => entry && typeof entry === "object" && entry.kind === "skill"
  );
}

function assetIdFromHash(hash) {
  const hex = hash.startsWith("sha256:") ? hash.slice("sha256:".length) : hash;
  return "asset_" + hex;
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

// A skill's SKILL.md whose YAML frontmatter carries the tool name + description.
const SKILL_MD = "---\nname: alpha-skill\ndescription: Alpha.\n---\n# Alpha\nUse the alpha behavior.\n";
const REF_MD = "# Alpha reference\nExtra file that rides in the same bundle.\n";
`;

describe("Tools.fromSkillUrl (installed package)", () => {
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

  it("ingests a URL skill end-to-end: download, unzip, name override, folder strip, dir<->url dedup", async () => {
    const script = CHILD_HARNESS + String.raw`
const { Aex, Tools, bundleSkillFiles, hashSkillBundle } = await import("@aexhq/sdk");
const { zipSync } = await import("fflate");
const enc = new TextEncoder();

function makeClient(archives) {
  const harness = makeFetch(archives);
  const client = new Aex({
    apiToken: "aex_skill_url_token",
    baseUrl: "https://example.invalid",
    fetch: harness.fetch
  });
  return { ...harness, client };
}

// ---- Happy path: download -> unzip -> build -> upload -> serialize ----
const happyFiles = { "SKILL.md": SKILL_MD };
const happyZip = bundleSkillFiles(happyFiles).zip;
const happyHash = await hashSkillBundle(happyZip);
const happyUrl = "https://skills.example.test/alpha.zip";
const happy = makeClient({ [happyUrl]: { status: 200, bytes: happyZip } });

const skill = await Tools.fromSkillUrl(happyUrl, { fetch: happy.fetch });
strictEqual(skill.isDraft, true);
strictEqual(skill.ref.kind, "draft");
strictEqual(skill.ref.name, "alpha-skill");
strictEqual(skill.ref.description, "Alpha.");
strictEqual(skill.ref.contentHash, happyHash);
// A draft skill-tool only becomes a wire ref once uploaded.
await expectReject("draft toJSON", async () => skill.toJSON(), /draft skill-tools cannot be JSON-serialised/);

await happy.client.sessions.create({
  model: "claude-haiku-4-5",
  tools: [skill],
  apiKeys: { anthropic: "sk-ant" }
});
const happyBody = onlyCreateBody(happy.calls);
strictEqual(happyBody.submission.tools.length, 1, "only the skill-tool rides submission.tools");
deepStrictEqual(skillToolEntries(happyBody), [
  { kind: "skill", assetId: assetIdFromHash(happyHash), name: "alpha-skill", description: "Alpha." }
]);
strictEqual(archiveFetches(happy.calls, "skills.example.test").length, 1);
strictEqual(presignCalls(happy.calls).length, 1);
strictEqual(storagePuts(happy.calls).length, 1);
strictEqual(finalizeCalls(happy.calls).length, 1);

// ---- { name } override: metadata-only, bytes (and asset) unchanged ----
const ovUrl = "https://skills.example.test/override.zip";
const ov = makeClient({ [ovUrl]: { status: 200, bytes: happyZip } });
const ovSkill = await Tools.fromSkillUrl(ovUrl, { fetch: ov.fetch, name: "renamed-skill" });
strictEqual(ovSkill.ref.name, "renamed-skill");
strictEqual(ovSkill.ref.description, "Alpha.");
strictEqual(ovSkill.ref.contentHash, happyHash, "name override does not change bundle bytes");
await ov.client.sessions.create({
  model: "claude-haiku-4-5",
  tools: [ovSkill],
  apiKeys: { anthropic: "sk-ant" }
});
deepStrictEqual(skillToolEntries(onlyCreateBody(ov.calls)), [
  { kind: "skill", assetId: assetIdFromHash(happyHash), name: "renamed-skill", description: "Alpha." }
]);

// ---- Top-level folder strip is transparent to the canonical asset ----
const rootFiles = { "SKILL.md": SKILL_MD, "reference.md": REF_MD };
const rootZip = bundleSkillFiles(rootFiles).zip;
const rootHash = await hashSkillBundle(rootZip);
// Every entry nested under a single top-level folder -> stripped on ingest.
const folderZip = zipSync({
  "my-skill/SKILL.md": enc.encode(SKILL_MD),
  "my-skill/reference.md": enc.encode(REF_MD)
});
const rootUrl = "https://skills.example.test/root.zip";
const folderUrl = "https://skills.example.test/folder.zip";
const strip = makeClient({
  [rootUrl]: { status: 200, bytes: rootZip },
  [folderUrl]: { status: 200, bytes: folderZip }
});
const rootSkill = await Tools.fromSkillUrl(rootUrl, { fetch: strip.fetch });
const folderSkill = await Tools.fromSkillUrl(folderUrl, { fetch: strip.fetch });
strictEqual(rootSkill.ref.contentHash, rootHash);
strictEqual(folderSkill.ref.contentHash, rootHash, "stripped folder yields the same canonical asset");
strictEqual(folderSkill.ref.contentHash, rootSkill.ref.contentHash);

// ---- Dir <-> URL dedup: identical local dir + URL skill share one asset ----
const dedupUrl = "https://skills.example.test/dedup.zip";
const dedupFiles = { "SKILL.md": SKILL_MD, "reference.md": REF_MD };
const dedupZip = bundleSkillFiles(dedupFiles).zip;
const dedupHash = await hashSkillBundle(dedupZip);
const dir = mkdtempSync(join(tmpdir(), "aex-skill-dir-"));
writeFileSync(join(dir, "SKILL.md"), SKILL_MD);
writeFileSync(join(dir, "reference.md"), REF_MD);
const dedup = makeClient({ [dedupUrl]: { status: 200, bytes: dedupZip } });
const dirSkill = await Tools.fromSkillDir(dir);
const urlSkill = await Tools.fromSkillUrl(dedupUrl, { fetch: dedup.fetch });
strictEqual(dirSkill.ref.contentHash, dedupHash, "local dir hashes to the canonical bundle");
strictEqual(urlSkill.ref.contentHash, dedupHash, "url skill hashes to the same canonical bundle");
await dedup.client.sessions.create({
  model: "claude-haiku-4-5",
  tools: [dirSkill, urlSkill],
  apiKeys: { anthropic: "sk-ant" }
});
const dedupEntries = skillToolEntries(onlyCreateBody(dedup.calls));
strictEqual(dedupEntries.length, 2);
strictEqual(dedupEntries[0].assetId, assetIdFromHash(dedupHash));
strictEqual(dedupEntries[1].assetId, assetIdFromHash(dedupHash));
strictEqual(dedupEntries[0].assetId, dedupEntries[1].assetId, "dir + url resolve to one shared asset");
// A single real upload despite two distinct instances: the 2nd presign is a
// content-address dedup hit, so exactly one PUT + one finalize.
strictEqual(storagePuts(dedup.calls).length, 1);
strictEqual(finalizeCalls(dedup.calls).length, 1);
const dedupPresigns = presignCalls(dedup.calls).length;

console.log(JSON.stringify({
  ok: true,
  happyName: skill.ref.name,
  happyDescription: skill.ref.description,
  happyPresigns: presignCalls(happy.calls).length,
  happyPuts: storagePuts(happy.calls).length,
  happyFinalizes: finalizeCalls(happy.calls).length,
  happyArchiveFetches: archiveFetches(happy.calls, "skills.example.test").length,
  overrideName: ovSkill.ref.name,
  folderStripMatches: folderSkill.ref.contentHash === rootHash,
  rootVsFolderSameHash: folderSkill.ref.contentHash === rootSkill.ref.contentHash,
  dedupSameAsset: dedupEntries[0].assetId === dedupEntries[1].assetId,
  dedupPuts: storagePuts(dedup.calls).length,
  dedupFinalizes: finalizeCalls(dedup.calls).length,
  dedupPresigns
}));
`;
    const result = await runChild(script, "skill-tool-from-url-ingest.mjs", 180_000);
    expect(result).toMatchObject({
      ok: true,
      happyName: "alpha-skill",
      happyDescription: "Alpha.",
      happyPresigns: 1,
      happyPuts: 1,
      happyFinalizes: 1,
      happyArchiveFetches: 1,
      overrideName: "renamed-skill",
      folderStripMatches: true,
      rootVsFolderSameHash: true,
      dedupSameAsset: true,
      dedupPuts: 1,
      dedupFinalizes: 1,
      // Dedup is per-instance (_cachedAssetId), so two distinct instances each
      // presign; the second is a server-side dedup hit (no PUT).
      dedupPresigns: 2
    });
  });

  it("enforces sha256 integrity on the fetched archive (prefixed + bare hex, mismatch, tamper)", async () => {
    const script = CHILD_HARNESS + String.raw`
const { Aex, Tools, bundleSkillFiles, hashSkillBundle } = await import("@aexhq/sdk");

const files = { "SKILL.md": SKILL_MD };
const zip = bundleSkillFiles(files).zip;
const contentHash = await hashSkillBundle(zip);          // "sha256:<hex>"
const hex = contentHash.slice("sha256:".length);          // bare hex
const url = "https://skills.example.test/integrity.zip";

// Correct hash, prefixed form -> accepted, and flows through to the wire.
const prefixed = makeFetch({ [url]: { status: 200, bytes: zip } });
const prefixedClient = new Aex({
  apiToken: "aex_integrity_token",
  baseUrl: "https://example.invalid",
  fetch: prefixed.fetch
});
const prefixedSkill = await Tools.fromSkillUrl(url, { fetch: prefixed.fetch, sha256: contentHash });
strictEqual(prefixedSkill.ref.contentHash, contentHash);
await prefixedClient.sessions.create({
  model: "claude-haiku-4-5",
  tools: [prefixedSkill],
  apiKeys: { anthropic: "sk-ant" }
});
deepStrictEqual(skillToolEntries(onlyCreateBody(prefixed.calls)), [
  { kind: "skill", assetId: assetIdFromHash(contentHash), name: "alpha-skill", description: "Alpha." }
]);

// Correct hash, bare-hex form -> also accepted (prefix is optional on input).
const bare = makeFetch({ [url]: { status: 200, bytes: zip } });
const bareSkill = await Tools.fromSkillUrl(url, { fetch: bare.fetch, sha256: hex });
strictEqual(bareSkill.ref.contentHash, contentHash);

// Wrong-but-well-formed hash -> integrity error, no upload attempted.
const wrongHex = hex === "0".repeat(64) ? "1".repeat(64) : "0".repeat(64);
const wrong = makeFetch({ [url]: { status: 200, bytes: zip } });
const wrongMsg = await expectReject(
  "wrong sha256",
  () => Tools.fromSkillUrl(url, { fetch: wrong.fetch, sha256: "sha256:" + wrongHex }),
  /archive integrity check failed/
);
strictEqual(presignCalls(wrong.calls).length, 0);

// Malformed hash (not 64 hex) -> rejected before hashing/unzip.
const malformed = makeFetch({ [url]: { status: 200, bytes: zip } });
await expectReject(
  "malformed sha256",
  () => Tools.fromSkillUrl(url, { fetch: malformed.fetch, sha256: "not-a-valid-hash" }),
  /sha256 must be 64 hex chars/
);

// Tampered bytes: serve a DIFFERENT valid skill zip but claim the original hash.
const otherZip = bundleSkillFiles({
  "SKILL.md": "---\nname: beta-skill\ndescription: Beta.\n---\n# Beta\n"
}).zip;
const tampered = makeFetch({ [url]: { status: 200, bytes: otherZip } });
await expectReject(
  "tampered bytes",
  () => Tools.fromSkillUrl(url, { fetch: tampered.fetch, sha256: contentHash }),
  /archive integrity check failed/
);
strictEqual(presignCalls(tampered.calls).length, 0);

console.log(JSON.stringify({
  ok: true,
  prefixedAssetId: skillToolEntries(onlyCreateBody(prefixed.calls))[0].assetId,
  bareHexAccepted: bareSkill.ref.contentHash === contentHash,
  wrongRejected: /integrity check failed/.test(wrongMsg),
  integrityRejectPresigns: presignCalls(wrong.calls).length + presignCalls(tampered.calls).length
}));
`;
    const result = await runChild(script, "skill-tool-from-url-integrity.mjs", 180_000);
    expect(result).toMatchObject({
      ok: true,
      prefixedAssetId: expect.stringMatching(/^asset_[0-9a-f]{64}$/),
      bareHexAccepted: true,
      wrongRejected: true,
      integrityRejectPresigns: 0
    });
  });

  it("rejects unfetchable, malformed, skill-less, empty, and slow archives with no session call", async () => {
    const script = CHILD_HARNESS + String.raw`
const { Tools, bundleSkillFiles } = await import("@aexhq/sdk");
const { zipSync } = await import("fflate");
const enc = new TextEncoder();

// HTTP 404 -> surfaced, no upload.
const notFound = makeFetch({ "https://skills.example.test/missing.zip": { status: 404 } });
await expectReject(
  "http 404",
  () => Tools.fromSkillUrl("https://skills.example.test/missing.zip", { fetch: notFound.fetch }),
  /returned HTTP 404/
);
strictEqual(presignCalls(notFound.calls).length, 0);

// HTTP 500 -> surfaced.
const serverErr = makeFetch({ "https://skills.example.test/boom.zip": { status: 500 } });
await expectReject(
  "http 500",
  () => Tools.fromSkillUrl("https://skills.example.test/boom.zip", { fetch: serverErr.fetch }),
  /returned HTTP 500/
);

// Body is not a zip.
const notZip = makeFetch({
  "https://skills.example.test/plain.txt": { status: 200, bytes: enc.encode("this is plainly not a zip archive") }
});
await expectReject(
  "not a zip",
  () => Tools.fromSkillUrl("https://skills.example.test/plain.txt", { fetch: notZip.fetch }),
  /could not unzip/
);

// A valid zip with NO SKILL.md anywhere (and no single top-level folder).
const noSkillZip = zipSync({
  "README.md": enc.encode("# not a skill\n"),
  "notes.txt": enc.encode("nothing here\n")
});
const noSkill = makeFetch({ "https://skills.example.test/noskill.zip": { status: 200, bytes: noSkillZip } });
await expectReject(
  "no SKILL.md",
  () => Tools.fromSkillUrl("https://skills.example.test/noskill.zip", { fetch: noSkill.fetch }),
  /must contain SKILL\.md at its root/
);

// Empty response body.
const empty = makeFetch({ "https://skills.example.test/empty.zip": { status: 200, bytes: new Uint8Array() } });
await expectReject(
  "empty body",
  () => Tools.fromSkillUrl("https://skills.example.test/empty.zip", { fetch: empty.fetch }),
  /is empty/
);

// timeoutMs exceeded: a fetch that never resolves, but honors the abort signal.
const hangingFetch = async (input, init = {}) =>
  await new Promise((_resolve, reject) => {
    const signal = init && init.signal;
    const onAbort = () => reject(new Error("The operation was aborted"));
    if (signal) {
      if (signal.aborted) {
        onAbort();
        return;
      }
      signal.addEventListener("abort", onAbort, { once: true });
    }
    // Otherwise never settles -> only the timeout abort ends it.
  });
const timedOutMsg = await expectReject(
  "timeout",
  () => Tools.fromSkillUrl("https://skills.example.test/slow.zip", { fetch: hangingFetch, timeoutMs: 50 }),
  /fetch failed for/
);

// A bad hash never reaches the URL: url is validated first.
await expectReject(
  "missing url",
  () => Tools.fromSkillUrl("", { fetch: notFound.fetch }),
  /url is required/
);

console.log(JSON.stringify({
  ok: true,
  notFoundPresigns: presignCalls(notFound.calls).length,
  timedOut: /fetch failed for/.test(timedOutMsg),
  totalArchiveAttempts:
    archiveFetches(notFound.calls, "skills.example.test").length +
    archiveFetches(serverErr.calls, "skills.example.test").length +
    archiveFetches(notZip.calls, "skills.example.test").length +
    archiveFetches(noSkill.calls, "skills.example.test").length +
    archiveFetches(empty.calls, "skills.example.test").length
}));
`;
    const result = await runChild(script, "skill-tool-from-url-errors.mjs", 120_000);
    expect(result).toMatchObject({
      ok: true,
      notFoundPresigns: 0,
      timedOut: true,
      totalArchiveAttempts: 5
    });
  });
});
