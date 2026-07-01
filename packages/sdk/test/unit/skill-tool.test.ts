/**
 * SDK shape tests for the skill-tool ingestion surface (`Tools.fromSkillDir` /
 * `Tools.fromSkillUrl`).
 *
 * A skill is ingested as a synthetic no-arg load-tool: the factories read a
 * skill folder/zip, lift the tool `name` + `description` from the SKILL.md YAML
 * frontmatter (an explicit `name` argument overrides the frontmatter), and
 * canonically bundle the bytes so a URL-sourced skill and the identical local
 * skill produce the same asset. The URL never reaches the wire.
 */
import { describe, expect, it } from "vitest";
import { mkdtempSync, rmSync, writeFileSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { zipSync } from "fflate";
import { SkillTool, Tools } from "../../src/index.js";

const TEXT = new TextEncoder();

/** A minimal SKILL.md with YAML frontmatter carrying name + description. */
function skillMd(name: string, description: string, body = "do the thing"): string {
  return `---\nname: ${name}\ndescription: ${description}\n---\n# ${name}\n\n${body}\n`;
}

function makeZip(files: Record<string, string>): Uint8Array {
  const zippable: Record<string, Uint8Array> = {};
  for (const [path, contents] of Object.entries(files)) {
    zippable[path] = TEXT.encode(contents);
  }
  return zipSync(zippable, { level: 0 });
}

/** A `fetch` stub that always returns `bytes` with the given status. */
function fetchReturning(bytes: Uint8Array, status = 200) {
  return async () => new Response(bytes, { status });
}

async function sha256Hex(bytes: Uint8Array): Promise<string> {
  const copy = new Uint8Array(bytes.byteLength);
  copy.set(bytes);
  const digest = await crypto.subtle.digest("SHA-256", copy.buffer);
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/** Pull the draft bundle out for inspection (internal method). */
function takeBundle(tool: SkillTool): {
  name: string;
  description: string;
  contentHash: string;
  bytes: Uint8Array;
} {
  return (
    tool as unknown as {
      _takeDraftBundle(): { name: string; description: string; contentHash: string; bytes: Uint8Array };
    }
  )._takeDraftBundle();
}

/** Await a promise expected to reject and return the thrown Error. */
async function rejection(promise: Promise<unknown>): Promise<Error> {
  try {
    await promise;
  } catch (err) {
    return err as Error;
  }
  throw new Error("expected the promise to reject, but it resolved");
}

/** Make a real temp skill directory containing the given files. */
function makeSkillDir(files: Record<string, string>): { dir: string; cleanup: () => void } {
  const dir = mkdtempSync(join(tmpdir(), "aex-skill-tool-"));
  for (const [rel, contents] of Object.entries(files)) {
    const abs = join(dir, rel);
    const slash = abs.lastIndexOf("/");
    const bslash = abs.lastIndexOf("\\");
    const parent = abs.slice(0, Math.max(slash, bslash));
    if (parent && parent !== dir) mkdirSync(parent, { recursive: true });
    writeFileSync(abs, contents, "utf8");
  }
  return { dir, cleanup: () => { try { rmSync(dir, { recursive: true, force: true }); } catch { /* best-effort */ } } };
}

describe("Tools.fromSkillUrl — frontmatter lifting", () => {
  it("lifts name + description from SKILL.md frontmatter into a draft skill-tool", async () => {
    const zip = makeZip({ "SKILL.md": skillMd("pdf-filler", "Fills PDF forms."), "lib/util.js": "x" });
    const tool = await Tools.fromSkillUrl("https://store.example.com/s.zip", {
      fetch: fetchReturning(zip)
    });
    expect(tool.isDraft).toBe(true);
    const bundle = takeBundle(tool);
    expect(bundle.name).toBe("pdf-filler");
    expect(bundle.description).toBe("Fills PDF forms.");
    expect(bundle.contentHash).toMatch(/^sha256:[0-9a-f]{64}$/);
  });

  it("lets an explicit name argument override the frontmatter name", async () => {
    const zip = makeZip({ "SKILL.md": skillMd("frontmatter-name", "A skill.") });
    const tool = await Tools.fromSkillUrl("https://x/s.zip", { name: "override-name", fetch: fetchReturning(zip) });
    expect(takeBundle(tool).name).toBe("override-name");
    expect(takeBundle(tool).description).toBe("A skill.");
  });

  it("strips surrounding quotes on a quoted description value", async () => {
    const md = `---\nname: quoted\ndescription: "Quoted: with a colon."\n---\n# quoted\n`;
    const tool = await Tools.fromSkillUrl("https://x/s.zip", { fetch: fetchReturning(makeZip({ "SKILL.md": md })) });
    expect(takeBundle(tool).description).toBe("Quoted: with a colon.");
  });

  it("throws when no name is available (no frontmatter name, no argument)", async () => {
    const md = `---\ndescription: Has a description but no name.\n---\n# untitled\n`;
    const err = await rejection(
      Tools.fromSkillUrl("https://x/s.zip", { fetch: fetchReturning(makeZip({ "SKILL.md": md })) })
    );
    expect(err.message).toMatch(/name is required/);
  });

  it("throws when the frontmatter has no description", async () => {
    const md = `---\nname: no-desc\n---\n# no-desc\n`;
    const err = await rejection(
      Tools.fromSkillUrl("https://x/s.zip", { fetch: fetchReturning(makeZip({ "SKILL.md": md })) })
    );
    expect(err.message).toMatch(/description is required/);
  });

  it("rejects an invalid explicit name", async () => {
    const zip = makeZip({ "SKILL.md": skillMd("ok", "desc") });
    await expect(
      Tools.fromSkillUrl("https://x/s.zip", { name: "Bad Name!", fetch: fetchReturning(zip) })
    ).rejects.toThrow(/name must match/);
  });

  it("rejects a name containing the reserved MCP separator", async () => {
    const md = `---\nname: bad__name\ndescription: nope\n---\n# x\n`;
    await expect(
      Tools.fromSkillUrl("https://x/s.zip", { fetch: fetchReturning(makeZip({ "SKILL.md": md })) })
    ).rejects.toThrow(/must not contain "__"/);
  });
});

describe("Tools.fromSkillUrl — root resolution & transport (reused fetch-archive mechanics)", () => {
  it("strips a single top-level folder that contains SKILL.md", async () => {
    const wrapped = makeZip({ "my-skill/SKILL.md": skillMd("t", "d"), "my-skill/lib/util.js": "x" });
    const flat = { "SKILL.md": skillMd("t", "d"), "lib/util.js": "x" };
    const a = await Tools.fromSkillUrl("https://x/s.zip", { fetch: fetchReturning(wrapped) });
    const dir = makeSkillDir(flat);
    try {
      const b = await Tools.fromSkillDir(dir.dir);
      // Stripping the wrapper must yield a byte-identical canonical bundle.
      expect(takeBundle(a).contentHash).toBe(takeBundle(b).contentHash);
    } finally {
      dir.cleanup();
    }
  });

  it("passes a correct sha256 integrity check", async () => {
    const zip = makeZip({ "SKILL.md": skillMd("t", "d") });
    const hash = await sha256Hex(zip);
    const tool = await Tools.fromSkillUrl("https://x/s.zip", {
      sha256: `sha256:${hash}`,
      fetch: fetchReturning(zip)
    });
    expect(tool.isDraft).toBe(true);
  });

  it("rejects a wrong sha256 before unzip", async () => {
    const zip = makeZip({ "SKILL.md": skillMd("t", "d") });
    await expect(
      Tools.fromSkillUrl("https://x/s.zip", { sha256: `sha256:${"0".repeat(64)}`, fetch: fetchReturning(zip) })
    ).rejects.toThrow(/integrity check failed/);
  });

  it("throws on non-2xx and redacts the signed-URL query string", async () => {
    const url = "https://store.example.com/s.zip?X-Sig=SECRETSIGNATURE&exp=123";
    const err = await rejection(
      Tools.fromSkillUrl(url, { fetch: fetchReturning(TEXT.encode("nope"), 403) })
    );
    expect(err.message).toContain("HTTP 403");
    expect(err.message).toContain("store.example.com/s.zip");
    expect(err.message).not.toContain("SECRETSIGNATURE");
  });

  it("throws when there is no SKILL.md anywhere", async () => {
    const zip = makeZip({ "readme.md": "x", "lib.js": "y" });
    await expect(
      Tools.fromSkillUrl("https://x/s.zip", { fetch: fetchReturning(zip) })
    ).rejects.toThrow(/SKILL\.md/);
  });
});

describe("Tools.fromSkillDir — local directory ingestion", () => {
  it("builds a draft skill-tool from a directory and matches the URL-sourced hash", async () => {
    const files = { "SKILL.md": skillMd("shared", "Shared skill."), "lib/util.js": "x" };
    const dir = makeSkillDir(files);
    try {
      const fromDir = await Tools.fromSkillDir(dir.dir);
      const fromUrl = await Tools.fromSkillUrl("https://x/s.zip", { fetch: fetchReturning(makeZip(files)) });
      expect(takeBundle(fromDir).name).toBe("shared");
      expect(takeBundle(fromDir).description).toBe("Shared skill.");
      expect(takeBundle(fromDir).contentHash).toBe(takeBundle(fromUrl).contentHash);
    } finally {
      dir.cleanup();
    }
  });
});

describe("SkillTool.toJSON", () => {
  it("refuses to serialise an un-uploaded draft", async () => {
    const zip = makeZip({ "SKILL.md": skillMd("t", "d") });
    const tool = await Tools.fromSkillUrl("https://x/s.zip", { fetch: fetchReturning(zip) });
    expect(() => tool.toJSON()).toThrow(/draft skill-tools cannot be JSON-serialised/);
  });
});
