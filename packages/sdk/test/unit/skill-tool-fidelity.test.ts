/**
 * Phase B2.5 — `Skill.fromDir` / `Tool.fromPath` fidelity: `.aexignore` + built-in
 * defaults (`node_modules/`, `.git/`) prune the upload, exec bits + symlinks are
 * captured into the `.aexmeta.json` sidecar, and — the load-bearing invariant — a
 * metadata-free dir stays BYTE-IDENTICAL to the pre-fidelity (plain files-map)
 * output so existing content-addressed dedup is preserved.
 *
 * Exec-bit capture is posix-gated (Windows `chmod` does not set exec bits); symlink
 * capture is capability-gated (some Windows hosts forbid symlink creation). These
 * are CONDITIONAL assertions, never silent skips.
 */
import { afterEach, describe, expect, it } from "vitest";
import { mkdtemp, mkdir, writeFile, chmod, symlink, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { unzipSync } from "fflate";
import { parseBundleManifest, RESERVED_META_ENTRY } from "@aexhq/contracts";
import { Skill } from "../../src/skill.js";
import { Tool } from "../../src/tool.js";
import { readDirectoryWithFidelity } from "../../src/node-fs.js";

const isPosix = process.platform !== "win32";
const tmpDirs: string[] = [];

afterEach(async () => {
  while (tmpDirs.length) await rm(tmpDirs.pop()!, { recursive: true, force: true }).catch(() => undefined);
});

async function makeDir(): Promise<string> {
  const d = await mkdtemp(join(tmpdir(), "aex-stfid-"));
  tmpDirs.push(d);
  return d;
}

async function trySymlink(target: string, path: string): Promise<boolean> {
  try {
    await symlink(target, path);
    return true;
  } catch {
    return false;
  }
}

function skillMd(name: string, description: string, body = "do the thing"): string {
  return ["---", `name: ${name}`, `description: ${description}`, "---", `# ${name}`, "", body, ""].join("\n");
}

const TOOL_MANIFEST = {
  name: "calendar_lookup",
  description: "Looks up calendar availability.",
  input_schema: { type: "object" as const, properties: {}, required: [] },
  entry: "src/index.js"
};

function skillZip(skill: Skill): Record<string, Uint8Array> {
  const bundle = skill._takeDraftBundle();
  if (!bundle) throw new Error("expected a draft skill");
  return unzipSync(bundle.bytes);
}

function skillBytes(skill: Skill): Uint8Array {
  const bundle = skill._takeDraftBundle();
  if (!bundle) throw new Error("expected a draft skill");
  return bundle.bytes;
}

function toolZip(tool: Tool): Record<string, Uint8Array> {
  const bundle = (tool as unknown as { _takeDraftBundle(): { bytes: Uint8Array } })._takeDraftBundle();
  return unzipSync(bundle.bytes);
}

function toolBytes(tool: Tool): Uint8Array {
  return (tool as unknown as { _takeDraftBundle(): { bytes: Uint8Array } })._takeDraftBundle().bytes;
}

// ---------------------------------------------------------------------------
// Skill.fromDir
// ---------------------------------------------------------------------------

describe("Skill.fromDir — .aexignore + defaults", () => {
  it("prunes node_modules/ and .git/ by default and honors .aexignore", async () => {
    const d = await makeDir();
    await writeFile(join(d, "SKILL.md"), skillMd("s", "A skill."));
    await writeFile(join(d, "keep.txt"), "k");
    await writeFile(join(d, "drop.log"), "d");
    await mkdir(join(d, "node_modules", "pkg"), { recursive: true });
    await writeFile(join(d, "node_modules", "pkg", "index.js"), "x");
    await mkdir(join(d, ".git"));
    await writeFile(join(d, ".git", "config"), "x");
    await writeFile(join(d, ".aexignore"), "*.log\n");

    const entries = skillZip(await Skill.fromDir(d));
    const names = Object.keys(entries).filter((n) => n !== RESERVED_META_ENTRY);
    expect(names.sort()).toEqual(["SKILL.md", "keep.txt"]);
    // The control file itself is never bundled; node_modules/.git are pruned.
    expect(entries[".aexignore"]).toBeUndefined();
    expect(Object.keys(entries).some((n) => n.startsWith("node_modules/"))).toBe(false);
  });
});

describe("Skill.fromDir — exec bits (posix-gated)", () => {
  it("captures the +x bit into the sidecar exec[] (posix); no sidecar otherwise", async () => {
    const d = await makeDir();
    await writeFile(join(d, "SKILL.md"), skillMd("s", "A skill."));
    await writeFile(join(d, "run.sh"), "#!/bin/sh\necho hi\n");
    await chmod(join(d, "run.sh"), 0o755);

    const entries = skillZip(await Skill.fromDir(d));
    if (isPosix) {
      const manifest = parseBundleManifest(entries[RESERVED_META_ENTRY]);
      expect(manifest?.exec).toEqual(["run.sh"]);
      // The sidecar is the LAST entry (appended after sorted content).
      const names = Object.keys(entries);
      expect(names[names.length - 1]).toBe(RESERVED_META_ENTRY);
    } else {
      expect(entries[RESERVED_META_ENTRY]).toBeUndefined();
    }
  });
});

describe("Skill.fromDir — symlinks (capability-gated)", () => {
  it("captures an in-tree symlink into the sidecar (not as a content entry)", async () => {
    const d = await makeDir();
    await writeFile(join(d, "SKILL.md"), skillMd("s", "A skill."));
    await writeFile(join(d, "real.txt"), "content");
    if (!(await trySymlink("real.txt", join(d, "link.txt")))) return; // host forbids symlinks

    const entries = skillZip(await Skill.fromDir(d));
    expect(entries["link.txt"]).toBeUndefined();
    expect(entries["real.txt"]).toBeDefined();
    const manifest = parseBundleManifest(entries[RESERVED_META_ENTRY]);
    expect(manifest?.symlinks).toEqual([{ path: "link.txt", target: "real.txt" }]);
  });

  it("captures an ESCAPING symlink VERBATIM (rejection is a restore-side decision)", async () => {
    const d = await makeDir();
    await writeFile(join(d, "SKILL.md"), skillMd("s", "A skill."));
    if (!(await trySymlink(join("..", "..", "etc", "passwd"), join(d, "escape")))) return;
    const manifest = parseBundleManifest(skillZip(await Skill.fromDir(d))[RESERVED_META_ENTRY]);
    const link = manifest?.symlinks.find((s) => s.path === "escape");
    expect(link).toBeDefined();
    expect(link!.target.replace(/\\/g, "/")).toContain("../../etc/passwd");
  });
});

describe("Skill.fromDir — no-metadata byte-identity (dedup continuity)", () => {
  it("a pure-content skill dir is byte-identical to Skill.fromFiles of the same map, with NO sidecar", async () => {
    const files = { "SKILL.md": skillMd("shared", "Shared."), "lib/util.js": "x" };
    const d = await makeDir();
    await writeFile(join(d, "SKILL.md"), files["SKILL.md"]);
    await mkdir(join(d, "lib"));
    await writeFile(join(d, "lib", "util.js"), files["lib/util.js"]);

    const fromDir = await Skill.fromDir(d, { name: "shared" });
    const fromFiles = await Skill.fromFiles({ name: "shared", files });

    // Byte-for-byte identical: the fidelity walk of a clean dir produces exactly
    // what the plain files-map path (today's output) produces.
    expect([...skillBytes(fromDir)]).toEqual([...skillBytes(fromFiles)]);
    expect(skillZip(fromDir)[RESERVED_META_ENTRY]).toBeUndefined();
  });
});

describe("Skill.fromFiles — meta threading (OS-independent sidecar plumbing)", () => {
  it("emits the sidecar (exec + symlinks) when meta is provided", async () => {
    const skill = await Skill.fromFiles({
      name: "s",
      files: { "SKILL.md": skillMd("s", "A skill."), "run.sh": "#!/bin/sh\n", "real.txt": "r" },
      meta: { exec: ["run.sh"], symlinks: [{ path: "link.txt", target: "real.txt" }] }
    });
    const entries = skillZip(skill);
    const manifest = parseBundleManifest(entries[RESERVED_META_ENTRY]);
    expect(manifest?.exec).toEqual(["run.sh"]);
    expect(manifest?.symlinks).toEqual([{ path: "link.txt", target: "real.txt" }]);
    // The sidecar is the LAST entry (appended after sorted content).
    const names = Object.keys(entries);
    expect(names[names.length - 1]).toBe(RESERVED_META_ENTRY);
  });

  it("emits NO sidecar for empty meta → byte-identical to no meta", async () => {
    const files = { "SKILL.md": skillMd("s", "A skill."), "a.txt": "a" };
    const withEmpty = await Skill.fromFiles({ name: "s", files, meta: { exec: [], symlinks: [] } });
    const without = await Skill.fromFiles({ name: "s", files });
    expect([...skillBytes(withEmpty)]).toEqual([...skillBytes(without)]);
    expect(skillZip(withEmpty)[RESERVED_META_ENTRY]).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// Tool.fromPath
// ---------------------------------------------------------------------------

async function writeToolDir(d: string): Promise<void> {
  await writeFile(join(d, "tool.json"), JSON.stringify(TOOL_MANIFEST));
  await mkdir(join(d, "src"), { recursive: true });
  await writeFile(join(d, "src", "index.js"), "export default async () => {}\n");
}

describe("Tool.fromPath — .aexignore + defaults + exec", () => {
  it("prunes node_modules/ and captures exec into the sidecar (posix)", async () => {
    const d = await makeDir();
    await writeToolDir(d);
    await writeFile(join(d, "run.sh"), "#!/bin/sh\n");
    await chmod(join(d, "run.sh"), 0o755);
    await mkdir(join(d, "node_modules", "dep"), { recursive: true });
    await writeFile(join(d, "node_modules", "dep", "x.js"), "x");

    const entries = toolZip(await Tool.fromPath(d));
    expect(Object.keys(entries).some((n) => n.startsWith("node_modules/"))).toBe(false);
    expect(entries["src/index.js"]).toBeDefined();
    // tool.json is re-added by the bundler from the manifest fields.
    expect(entries["tool.json"]).toBeDefined();
    if (isPosix) {
      const manifest = parseBundleManifest(entries[RESERVED_META_ENTRY]);
      expect(manifest?.exec).toContain("run.sh");
    }
  });
});

describe("Tool.fromPath — no-metadata byte-identity (dedup continuity)", () => {
  it("a pure-content tool dir is byte-identical to Tool.fromFiles of the same map, with NO sidecar", async () => {
    const d = await makeDir();
    await writeToolDir(d);

    const fromPath = await Tool.fromPath(d);
    const fromFiles = await Tool.fromFiles({
      ...TOOL_MANIFEST,
      files: { "src/index.js": "export default async () => {}\n" }
    });

    expect([...toolBytes(fromPath)]).toEqual([...toolBytes(fromFiles)]);
    expect(toolZip(fromPath)[RESERVED_META_ENTRY]).toBeUndefined();
  });
});

describe("Tool.fromFiles — meta threading (OS-independent sidecar plumbing)", () => {
  it("emits the sidecar (exec) when meta is provided", async () => {
    const tool = await Tool.fromFiles({
      ...TOOL_MANIFEST,
      files: { "src/index.js": "export default async () => {}\n", "run.sh": "#!/bin/sh\n" },
      meta: { exec: ["run.sh"], symlinks: [] }
    });
    const entries = toolZip(tool);
    const manifest = parseBundleManifest(entries[RESERVED_META_ENTRY]);
    expect(manifest?.exec).toEqual(["run.sh"]);
    const names = Object.keys(entries);
    expect(names[names.length - 1]).toBe(RESERVED_META_ENTRY);
  });

  it("emits NO sidecar for empty meta → byte-identical to no meta", async () => {
    const files = { "src/index.js": "export default async () => {}\n" };
    const withEmpty = await Tool.fromFiles({ ...TOOL_MANIFEST, files, meta: { exec: [], symlinks: [] } });
    const without = await Tool.fromFiles({ ...TOOL_MANIFEST, files });
    expect([...toolBytes(withEmpty)]).toEqual([...toolBytes(without)]);
    expect(toolZip(withEmpty)[RESERVED_META_ENTRY]).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// dropped/ignored summary — surfaced (non-silent), mirroring the File walk.
// ---------------------------------------------------------------------------

describe("readDirectoryWithFidelity — non-silent ignore summary", () => {
  it("surfaces the ignored-count and excludes pruned paths from the files map", async () => {
    const d = await makeDir();
    await writeFile(join(d, "a.txt"), "a");
    await mkdir(join(d, "node_modules", "pkg"), { recursive: true });
    await writeFile(join(d, "node_modules", "pkg", "index.js"), "x");
    await mkdir(join(d, ".git"));
    await writeFile(join(d, ".git", "config"), "x");

    const res = await readDirectoryWithFidelity(d);
    expect(Object.keys(res.files)).toEqual(["a.txt"]);
    // node_modules/ and .git/ are each pruned wholesale → a non-zero ignored count.
    expect(res.ignoredCount).toBeGreaterThan(0);
    expect(res.meta.exec ?? []).toEqual([]);
    expect(res.meta.symlinks ?? []).toEqual([]);
  });
});
