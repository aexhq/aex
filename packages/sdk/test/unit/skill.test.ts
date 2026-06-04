/**
 * SDK shape tests for Skill.fromUrl.
 *
 * Skill.fromUrl fetches a zip-archived skill over a (signed) URL in the SDK
 * process, resolves the skill root (stripping a single wrapper directory when
 * that exposes SKILL.md), and reduces it to the same files map as
 * Skill.fromFiles — so a URL-sourced skill and the identical local skill
 * produce the same canonical asset. The URL never reaches the wire.
 */
import { describe, expect, it } from "vitest";
import { zipSync } from "fflate";
import { Skill } from "../../src/skill.js";

const TEXT = new TextEncoder();

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
function takeBundle(skill: Skill): { name: string; contentHash: string; bytes: Uint8Array } {
  const bundle = (skill as unknown as {
    _takeDraftBundle(): { name: string; contentHash: string; bytes: Uint8Array };
  })._takeDraftBundle();
  return bundle;
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

describe("Skill.fromUrl — happy path", () => {
  it("fetches a root-SKILL.md zip and builds a draft", async () => {
    const zip = makeZip({ "SKILL.md": "# hi", "lib/util.js": "x" });
    const skill = await Skill.fromUrl("https://store.example.com/s.zip", {
      name: "my-tool",
      fetch: fetchReturning(zip)
    });
    expect(skill.isDraft).toBe(true);
    const bundle = takeBundle(skill);
    expect(bundle.name).toBe("my-tool");
    expect(bundle.contentHash).toMatch(/^sha256:[0-9a-f]{64}$/);
  });

  it("produces the same canonical hash as Skill.fromFiles (cross-source dedup)", async () => {
    const files = { "SKILL.md": "# hi", "lib/util.js": "x" };
    const fromUrl = await Skill.fromUrl("https://store.example.com/s.zip", {
      name: "t",
      fetch: fetchReturning(makeZip(files))
    });
    const fromFiles = await Skill.fromFiles({ name: "t", files });
    expect(takeBundle(fromUrl).contentHash).toBe(takeBundle(fromFiles).contentHash);
  });

  it("strips a single top-level folder that contains SKILL.md", async () => {
    const wrapped = makeZip({ "my-skill/SKILL.md": "# hi", "my-skill/lib/util.js": "x" });
    const flat = { "SKILL.md": "# hi", "lib/util.js": "x" };
    const a = await Skill.fromUrl("https://x/s.zip", { name: "t", fetch: fetchReturning(wrapped) });
    const b = await Skill.fromFiles({ name: "t", files: flat });
    // Stripping the wrapper must yield byte-identical canonical bundle.
    expect(takeBundle(a).contentHash).toBe(takeBundle(b).contentHash);
  });
});

describe("Skill.fromUrl — root resolution validation", () => {
  it("throws when the single top-level folder has no SKILL.md", async () => {
    const zip = makeZip({ "my-skill/readme.md": "x" });
    await expect(
      Skill.fromUrl("https://x/s.zip", { name: "t", fetch: fetchReturning(zip) })
    ).rejects.toThrow(/SKILL\.md.*my-skill\/|my-skill\//);
  });

  it("does not guess when multiple top-level entries and none expose root SKILL.md", async () => {
    const zip = makeZip({ "a/SKILL.md": "x", "b/file": "y" });
    const err = await rejection(
      Skill.fromUrl("https://x/s.zip", { name: "t", fetch: fetchReturning(zip) })
    );
    expect(err.message).toMatch(/SKILL\.md/);
    expect(err.message).toContain("a/");
    expect(err.message).toContain("b/");
  });

  it("does not strip more than one level", async () => {
    const zip = makeZip({ "deep/nested/SKILL.md": "x" });
    await expect(
      Skill.fromUrl("https://x/s.zip", { name: "t", fetch: fetchReturning(zip) })
    ).rejects.toThrow(/SKILL\.md/);
  });

  it("throws when there is no SKILL.md anywhere", async () => {
    const zip = makeZip({ "readme.md": "x", "lib.js": "y" });
    await expect(
      Skill.fromUrl("https://x/s.zip", { name: "t", fetch: fetchReturning(zip) })
    ).rejects.toThrow(/SKILL\.md/);
  });

  it("throws on an empty archive (no files)", async () => {
    const zip = zipSync({}, { level: 0 });
    await expect(
      Skill.fromUrl("https://x/s.zip", { name: "t", fetch: fetchReturning(zip) })
    ).rejects.toThrow(/no files|empty/i);
  });
});

describe("Skill.fromUrl — integrity, transport, and input guards", () => {
  it("passes a correct sha256 integrity check", async () => {
    const zip = makeZip({ "SKILL.md": "# hi" });
    const hash = await sha256Hex(zip);
    const skill = await Skill.fromUrl("https://x/s.zip", {
      name: "t",
      sha256: `sha256:${hash}`,
      fetch: fetchReturning(zip)
    });
    expect(skill.isDraft).toBe(true);
  });

  it("rejects a wrong sha256 before unzip", async () => {
    const zip = makeZip({ "SKILL.md": "# hi" });
    await expect(
      Skill.fromUrl("https://x/s.zip", {
        name: "t",
        sha256: `sha256:${"0".repeat(64)}`,
        fetch: fetchReturning(zip)
      })
    ).rejects.toThrow(/integrity check failed/);
  });

  it("throws on non-2xx and redacts the signed-URL query string", async () => {
    const url = "https://store.example.com/s.zip?X-Sig=SECRETSIGNATURE&exp=123";
    const err = await rejection(
      Skill.fromUrl(url, { name: "t", fetch: fetchReturning(TEXT.encode("nope"), 403) })
    );
    expect(err.message).toContain("HTTP 403");
    expect(err.message).toContain("store.example.com/s.zip");
    expect(err.message).not.toContain("SECRETSIGNATURE");
  });

  it("throws a clear error when the body is not a zip", async () => {
    const notZip = TEXT.encode("this is not a zip file at all");
    await expect(
      Skill.fromUrl("https://x/s.zip", { name: "t", fetch: fetchReturning(notZip) })
    ).rejects.toThrow(/unzip|\.zip/i);
  });

  it("rejects an invalid name before fetching", async () => {
    let called = false;
    const fetch = (async () => {
      called = true;
      return new Response(new Uint8Array());
    }) as unknown as typeof globalThis.fetch;
    await expect(
      Skill.fromUrl("https://x/s.zip", { name: "Bad Name!", fetch })
    ).rejects.toThrow(/name must match/);
    expect(called).toBe(false);
  });
});

describe("Skill.fromCatalog — reference an uploaded catalog skill", () => {
  const HASH = "a".repeat(64);

  it("builds a non-draft asset ref from a ready catalog record", () => {
    const skill = Skill.fromCatalog({ name: "my-tool", hash: `sha256:${HASH}` });
    expect(skill.isDraft).toBe(false);
    expect(skill.toJSON()).toEqual({ kind: "asset", assetId: `asset_${HASH}`, name: "my-tool" });
  });

  it("accepts a bare hex hash too", () => {
    expect(Skill.fromCatalog({ name: "t", hash: HASH }).toJSON()).toMatchObject({ assetId: `asset_${HASH}` });
  });

  it("rejects a pending record with no content hash", () => {
    expect(() => Skill.fromCatalog({ name: "t", hash: null })).toThrow(/sha256|ready/);
  });

  it("rejects an invalid name", () => {
    expect(() => Skill.fromCatalog({ name: "Bad Name!", hash: `sha256:${HASH}` })).toThrow(/name must match/);
  });
})
