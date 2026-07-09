/**
 * Unit tests for the first-class `Skill` surface: factories, name derivation,
 * the reserved/`__`/pattern/length rejects across every factory, and the
 * `upload()` upsert (draft → asset + registry PUT; identical-bytes no-op +
 * instance-cache reuse; changed bytes overwrite; upload-on-uploaded throws).
 */
import { describe, expect, it } from "vitest";
import { mkdtempSync, rmSync, writeFileSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { zipSync } from "fflate";
import { Skill, SKILL_BUNDLE_LIMITS } from "../../src/index.js";

const TEXT = new TextEncoder();

function skillMd(name: string | undefined, description: string | undefined, body = "do the thing"): string {
  const lines = ["---"];
  if (name !== undefined) lines.push(`name: ${name}`);
  if (description !== undefined) lines.push(`description: ${description}`);
  lines.push("---", `# ${name ?? "skill"}`, "", body, "");
  return lines.join("\n");
}

function makeZip(files: Record<string, string>): Uint8Array {
  const zippable: Record<string, Uint8Array> = {};
  for (const [path, contents] of Object.entries(files)) zippable[path] = TEXT.encode(contents);
  return zipSync(zippable, { level: 0 });
}

function fetchReturning(bytes: Uint8Array, status = 200) {
  return async () => new Response(bytes, { status });
}

/** Create a temp skill dir with a controllable basename (subdir under a temp root). */
function makeNamedSkillDir(basename: string, files: Record<string, string>): { dir: string; cleanup: () => void } {
  const root = mkdtempSync(join(tmpdir(), "aex-skill-"));
  const dir = join(root, basename);
  mkdirSync(dir, { recursive: true });
  for (const [rel, contents] of Object.entries(files)) {
    const abs = join(dir, rel);
    const parent = abs.slice(0, Math.max(abs.lastIndexOf("/"), abs.lastIndexOf("\\")));
    if (parent && parent !== dir) mkdirSync(parent, { recursive: true });
    writeFileSync(abs, contents, "utf8");
  }
  return { dir, cleanup: () => { try { rmSync(root, { recursive: true, force: true }); } catch { /* best effort */ } } };
}

interface UpsertArgs { name: string; contentHash: string; description: string; sizeBytes: number }
function makeUploader() {
  const assetCalls: Array<{ hash: string; bytes: Uint8Array }> = [];
  const upsertCalls: UpsertArgs[] = [];
  const uploader = {
    async _uploadAsset(a: { bytes: Uint8Array; hash: string; contentType?: string }) {
      assetCalls.push({ hash: a.hash, bytes: a.bytes });
      return { assetId: `asset_${a.hash.slice("sha256:".length)}` };
    },
    async _upsertSkill(a: UpsertArgs) {
      upsertCalls.push(a);
      return { updated: true };
    }
  };
  return { uploader, assetCalls, upsertCalls };
}

describe("Skill — name derivation", () => {
  it("lifts name + description from SKILL.md frontmatter", async () => {
    const skill = await Skill.fromFiles({ files: { "SKILL.md": skillMd("pdf-filler", "Fills PDF forms.") } });
    expect(skill.name).toBe("pdf-filler");
    expect(skill.description).toBe("Fills PDF forms.");
    expect(skill.isDraft).toBe(true);
  });

  it("lets an explicit { name } override the frontmatter name", async () => {
    const skill = await Skill.fromFiles({ name: "override", files: { "SKILL.md": skillMd("frontmatter", "A skill.") } });
    expect(skill.name).toBe("override");
    expect(skill.description).toBe("A skill.");
  });

  it("falls back to the slugified directory basename for fromDir", async () => {
    const d = makeNamedSkillDir("My Cool Skill", { "SKILL.md": skillMd(undefined, "No name in frontmatter.") });
    try {
      const skill = await Skill.fromDir(d.dir);
      expect(skill.name).toBe("my-cool-skill");
    } finally {
      d.cleanup();
    }
  });

  it("slugifies a unicode/punctuation basename to the name pattern", async () => {
    const d = makeNamedSkillDir("Rép0rt__Writer!!", { "SKILL.md": skillMd(undefined, "desc") });
    try {
      const skill = await Skill.fromDir(d.dir);
      // Non-[a-z0-9] sessions collapse to single '-', so the reserved '__' can't survive.
      expect(skill.name).toMatch(/^[a-z0-9][a-z0-9_-]{0,127}$/);
      expect(skill.name.includes("__")).toBe(false);
      expect(skill.name).toBe("r-p0rt-writer");
    } finally {
      d.cleanup();
    }
  });

  it("throws when no name is available anywhere (fromFiles, no frontmatter name)", async () => {
    await expect(Skill.fromFiles({ files: { "SKILL.md": skillMd(undefined, "Has desc, no name.") } })).rejects.toThrow(
      /name is required/
    );
  });

  it("throws when a dir basename slugifies to empty and there is no frontmatter name", async () => {
    const d = makeNamedSkillDir("___", { "SKILL.md": skillMd(undefined, "desc") });
    try {
      await expect(Skill.fromDir(d.dir)).rejects.toThrow(/name is required/);
    } finally {
      d.cleanup();
    }
  });
});

describe("Skill — validation rejects (across factories)", () => {
  it("rejects an invalid explicit name pattern", async () => {
    await expect(Skill.fromContent(skillMd("ok", "d"), { name: "Bad Name!" })).rejects.toThrow(/must match/);
  });

  it("rejects a leading dash", async () => {
    await expect(Skill.fromContent(skillMd(undefined, "d"), { name: "-lead" })).rejects.toThrow(/must match/);
  });

  it("rejects the reserved MCP separator '__' (frontmatter and explicit)", async () => {
    await expect(Skill.fromFiles({ files: { "SKILL.md": skillMd("bad__name", "d") } })).rejects.toThrow(/"__"/);
    await expect(Skill.fromContent(skillMd(undefined, "d"), { name: "a__b" })).rejects.toThrow(/"__"/);
  });

  it("rejects reserved names 'skills' and 'skill' from every factory", async () => {
    await expect(Skill.fromContent(skillMd(undefined, "d"), { name: "skills" })).rejects.toThrow(/reserved/);
    await expect(Skill.fromContent(skillMd(undefined, "d"), { name: "skill" })).rejects.toThrow(/reserved/);
    await expect(Skill.fromFiles({ files: { "SKILL.md": skillMd("skills", "d") } })).rejects.toThrow(/reserved/);
    await expect(
      Skill.fromBytes({ zip: makeZip({ "SKILL.md": skillMd("skill", "d") }) })
    ).rejects.toThrow(/reserved/);
  });

  it("rejects a name longer than 128 chars", async () => {
    const long = "a".repeat(129);
    await expect(Skill.fromContent(skillMd(undefined, "d"), { name: long })).rejects.toThrow(/must match/);
    // 128 is exactly the ceiling and must pass.
    const ok = "a".repeat(128);
    const skill = await Skill.fromContent(skillMd(undefined, "d"), { name: ok });
    expect(skill.name).toBe(ok);
  });

  it("rejects a missing/empty description", async () => {
    await expect(Skill.fromFiles({ files: { "SKILL.md": skillMd("no-desc", undefined) } })).rejects.toThrow(
      /description is required/
    );
  });

  it("rejects a description longer than 2048 chars", async () => {
    await expect(
      Skill.fromFiles({ files: { "SKILL.md": skillMd("big", "x".repeat(2049)) } })
    ).rejects.toThrow(/description must be <= 2048/);
  });

  it("rejects a bundle with no root SKILL.md", async () => {
    await expect(Skill.fromFiles({ files: { "readme.md": "hi" } })).rejects.toThrow(/SKILL\.md/);
    await expect(Skill.fromBytes({ zip: makeZip({ "readme.md": "hi" }) })).rejects.toThrow(/SKILL\.md/);
  });
});

describe("Skill — factory equivalence + fromUrl", () => {
  it("fromDir, fromFiles, and fromUrl of the same bytes produce the same contentHash", async () => {
    const files = { "SKILL.md": skillMd("shared", "Shared."), "lib/util.js": "x" };
    const d = makeNamedSkillDir("shared-dir", files);
    try {
      const fromDir = await Skill.fromDir(d.dir, { name: "shared" });
      const fromFiles = await Skill.fromFiles({ name: "shared", files });
      const fromUrl = await Skill.fromUrl("https://x/s.zip", { name: "shared", fetch: fetchReturning(makeZip(files)) });
      const hashOf = (s: Skill) => (s.ref.kind === "draft" ? s.ref.contentHash : "");
      expect(hashOf(fromDir)).toBe(hashOf(fromFiles));
      expect(hashOf(fromFiles)).toBe(hashOf(fromUrl));
    } finally {
      d.cleanup();
    }
  });

  it("fromContent builds a single-file skill", async () => {
    const skill = await Skill.fromContent(skillMd("inline", "Inline skill."));
    expect(skill.name).toBe("inline");
    expect(skill.description).toBe("Inline skill.");
  });

  it("aborts a URL archive response body when timeoutMs expires after headers", async () => {
    const fetch = async (_input: RequestInfo | URL, init?: RequestInit) =>
      new Response(
        new ReadableStream<Uint8Array>({
          pull() {
            return new Promise<void>((_resolve, reject) => {
              const signal = init?.signal;
              const rejectAbort = () => reject(new Error("body aborted"));
              if (signal?.aborted) {
                rejectAbort();
                return;
              }
              signal?.addEventListener("abort", rejectAbort, { once: true });
            });
          }
        }),
        { status: 200 }
      );

    let guard: ReturnType<typeof setTimeout> | undefined;
    const didNotAbort = new Promise<never>((_resolve, reject) => {
      guard = setTimeout(() => reject(new Error("body read did not abort")), 500);
    });
    try {
      await expect(
        Promise.race([
          Skill.fromUrl("https://x/slow.zip", { name: "slow", fetch, timeoutMs: 20 }),
          didNotAbort
        ])
      ).rejects.toThrow(/fetch failed for https:\/\/x\/slow\.zip/);
    } finally {
      if (guard) clearTimeout(guard);
    }
  });

  it("stops reading a no-content-length URL body once the compressed cap is exceeded", async () => {
    let reads = 0;
    let cancelled = false;
    const oversizedChunk = { byteLength: SKILL_BUNDLE_LIMITS.maxCompressedBytes + 1 } as Uint8Array;

    const fetch = async () =>
      ({
        ok: true,
        status: 200,
        headers: new Headers(),
        body: {
          getReader() {
            return {
              async read() {
                reads += 1;
                return { done: false, value: oversizedChunk };
              },
              async cancel() {
                cancelled = true;
              }
            };
          }
        }
      }) as Response;

    await expect(Skill.fromUrl("https://x/too-big.zip", { name: "too-big", fetch })).rejects.toThrow(
      new RegExp(`exceeding the ${SKILL_BUNDLE_LIMITS.maxCompressedBytes}-byte compressed cap`)
    );
    expect(reads).toBe(1);
    expect(cancelled).toBe(true);
  });
});

describe("Skill.toJSON", () => {
  it("refuses to serialise an un-uploaded draft", async () => {
    const skill = await Skill.fromContent(skillMd("draft", "d"));
    expect(() => skill.toJSON()).toThrow(/draft skill cannot be JSON-serialised/);
  });

  it("serialises an uploaded skill to a by-name ref", async () => {
    const { uploader } = makeUploader();
    const uploaded = await (await Skill.fromContent(skillMd("done", "d"))).upload(uploader);
    expect(uploaded.toJSON()).toEqual({ kind: "skill", name: "done" });
  });
});

describe("Skill.upload — workspace upsert", () => {
  it("stages bytes then PUTs the registry entry and returns a by-name ref", async () => {
    const { uploader, assetCalls, upsertCalls } = makeUploader();
    const draft = await Skill.fromContent(skillMd("rules", "Keep it short."));
    const contentHash = draft.ref.kind === "draft" ? draft.ref.contentHash : "";
    const uploaded = await draft.upload(uploader);

    expect(assetCalls).toHaveLength(1);
    expect(assetCalls[0]!.hash).toBe(contentHash);
    expect(upsertCalls).toHaveLength(1);
    expect(upsertCalls[0]).toEqual({
      name: "rules",
      contentHash,
      description: "Keep it short.",
      sizeBytes: assetCalls[0]!.bytes.byteLength
    });
    expect(uploaded.isDraft).toBe(false);
    expect(uploaded.ref).toEqual({ kind: "skill", name: "rules" });
  });

  it("is idempotent on the same instance: a second upload skips both round-trips (instance cache)", async () => {
    const { uploader, assetCalls, upsertCalls } = makeUploader();
    const draft = await Skill.fromContent(skillMd("rules", "d"));
    await draft.upload(uploader);
    await draft.upload(uploader);
    expect(assetCalls).toHaveLength(1);
    expect(upsertCalls).toHaveLength(1);
  });

  it("uploads changed bytes under the same name (overwrite path)", async () => {
    const { uploader, assetCalls, upsertCalls } = makeUploader();
    const a = await Skill.fromContent(skillMd("rules", "d", "body A"));
    const b = await Skill.fromContent(skillMd("rules", "d", "body B"));
    const hashA = a.ref.kind === "draft" ? a.ref.contentHash : "";
    const hashB = b.ref.kind === "draft" ? b.ref.contentHash : "";
    expect(hashA).not.toBe(hashB);
    await a.upload(uploader);
    await b.upload(uploader);
    expect(assetCalls.map((c) => c.hash)).toEqual([hashA, hashB]);
    expect(upsertCalls.map((c) => c.name)).toEqual(["rules", "rules"]);
    expect(upsertCalls.map((c) => c.contentHash)).toEqual([hashA, hashB]);
  });

  it("throws when uploading an already-uploaded (non-draft) skill", async () => {
    const { uploader } = makeUploader();
    const uploaded = await (await Skill.fromContent(skillMd("rules", "d"))).upload(uploader);
    await expect(uploaded.upload(uploader)).rejects.toThrow(/only draft skills can be uploaded/);
  });
});
