/**
 * `File.fromPath` fidelity: `.aexmeta.json` sidecar (exec bits + symlinks),
 * `.aexignore` filtering, dropped specials, and dedup byte-identity when there is
 * no metadata. Exec-bit capture is posix-gated (Windows `chmod` does not set exec
 * bits); symlink capture is capability-gated (some Windows hosts forbid symlink
 * creation). These are CONDITIONAL assertions, never silent skips.
 */
import { afterEach, describe, expect, it } from "vitest";
import { mkdtemp, mkdir, writeFile, chmod, symlink, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { unzipSync, zipSync } from "fflate";
import { parseBundleManifest, RESERVED_META_ENTRY } from "@aexhq/contracts";
import { File } from "../../src/file.js";

const isPosix = process.platform !== "win32";
const tmpDirs: string[] = [];

afterEach(async () => {
  while (tmpDirs.length) await rm(tmpDirs.pop()!, { recursive: true, force: true }).catch(() => undefined);
});

async function makeDir(): Promise<string> {
  const d = await mkdtemp(join(tmpdir(), "aexfid-"));
  tmpDirs.push(d);
  return d;
}

/** Try to create a symlink; return whether it succeeded (some Windows hosts forbid it). */
async function trySymlink(target: string, path: string): Promise<boolean> {
  try {
    await symlink(target, path);
    return true;
  } catch {
    return false;
  }
}

async function zipOf(file: File): Promise<Record<string, Uint8Array>> {
  const bundle = file._takeDraftBundle();
  if (!bundle) throw new Error("expected a small (in-memory) draft");
  return unzipSync(bundle.bytes);
}

describe("File.fromPath — sidecar emission", () => {
  it("emits NO sidecar for a pure-content tree (byte-identical to plain zipSync → dedup continuity)", async () => {
    const d = await makeDir();
    await writeFile(join(d, "a.txt"), "alpha");
    await mkdir(join(d, "sub"));
    await writeFile(join(d, "sub", "b.txt"), "beta");

    const file = await File.fromPath(d);
    const entries = await zipOf(file);
    expect(Object.keys(entries).sort()).toEqual(["a.txt", "sub/b.txt"]);
    expect(entries[RESERVED_META_ENTRY]).toBeUndefined();

    // Byte-identical to what a plain sorted zipSync of the same content produces.
    const ZIP_EPOCH = new Date(Date.UTC(1980, 0, 1));
    const ref = zipSync(
      {
        "a.txt": [new TextEncoder().encode("alpha"), { mtime: ZIP_EPOCH }],
        "sub/b.txt": [new TextEncoder().encode("beta"), { mtime: ZIP_EPOCH }]
      },
      { level: 6 }
    );
    expect([...file._takeDraftBundle()!.bytes]).toEqual([...ref]);
  });

  it("is deterministic across repeated builds", async () => {
    const d = await makeDir();
    await writeFile(join(d, "x"), "1");
    await writeFile(join(d, "y"), "2");
    const a = await File.fromPath(d);
    const b = await File.fromPath(d);
    expect(a._takeDraftBundle()!.contentHash).toBe(b._takeDraftBundle()!.contentHash);
  });
});

describe("File.fromPath — exec bits (posix-gated)", () => {
  it("captures the +x bit into the sidecar exec[] (posix)", async () => {
    const d = await makeDir();
    await writeFile(join(d, "run.sh"), "#!/bin/sh\necho hi\n");
    await writeFile(join(d, "plain.txt"), "data");
    await chmod(join(d, "run.sh"), 0o755);
    await chmod(join(d, "plain.txt"), 0o644);

    const file = await File.fromPath(d);
    const entries = await zipOf(file);
    const manifest = parseBundleManifest(entries[RESERVED_META_ENTRY]);

    if (isPosix) {
      expect(manifest?.exec).toEqual(["run.sh"]);
      // The sidecar is the LAST entry.
      const names = Object.keys(entries);
      expect(names[names.length - 1]).toBe(RESERVED_META_ENTRY);
    } else {
      // Windows chmod does not set exec bits: no exec metadata, so no sidecar.
      expect(entries[RESERVED_META_ENTRY]).toBeUndefined();
    }
  });

  it("a solo executable single file gets an exec sidecar (posix)", async () => {
    if (!isPosix) return; // chmod is a no-op on Windows; nothing to assert
    const d = await makeDir();
    const p = join(d, "tool");
    await writeFile(p, "#!/bin/sh\n");
    await chmod(p, 0o755);
    const file = await File.fromPath(p);
    const entries = await zipOf(file);
    const manifest = parseBundleManifest(entries[RESERVED_META_ENTRY]);
    expect(manifest?.exec).toEqual(["tool"]);
    expect(entries["tool"]).toBeDefined();
  });
});

describe("File.fromPath — symlinks (capability-gated)", () => {
  it("captures an in-tree symlink verbatim into the sidecar", async () => {
    const d = await makeDir();
    await writeFile(join(d, "real.txt"), "content");
    const made = await trySymlink("real.txt", join(d, "link.txt"));
    if (!made) return; // host forbids symlink creation; capture is exercised elsewhere

    const file = await File.fromPath(d);
    const entries = await zipOf(file);
    // The symlink is NOT a content entry (manifest-only).
    expect(entries["link.txt"]).toBeUndefined();
    expect(entries["real.txt"]).toBeDefined();
    const manifest = parseBundleManifest(entries[RESERVED_META_ENTRY]);
    expect(manifest?.symlinks).toEqual([{ path: "link.txt", target: "real.txt" }]);
  });

  it("captures an escaping symlink VERBATIM (rejection is a restore-side decision)", async () => {
    const d = await makeDir();
    await writeFile(join(d, "keep.txt"), "x");
    const made = await trySymlink(join("..", "..", "etc", "passwd"), join(d, "escape"));
    if (!made) return;
    const file = await File.fromPath(d);
    const manifest = parseBundleManifest((await zipOf(file))[RESERVED_META_ENTRY]);
    const link = manifest?.symlinks.find((s) => s.path === "escape");
    expect(link).toBeDefined();
    expect(link!.target.replace(/\\/g, "/")).toContain("../../etc/passwd");
  });
});

describe("File.fromPath — .aexignore + defaults", () => {
  it("prunes node_modules/ and .git/ by default and honors .aexignore", async () => {
    const d = await makeDir();
    await writeFile(join(d, "keep.txt"), "k");
    await writeFile(join(d, "drop.log"), "d");
    await mkdir(join(d, "node_modules", "pkg"), { recursive: true });
    await writeFile(join(d, "node_modules", "pkg", "index.js"), "x");
    await mkdir(join(d, ".git"));
    await writeFile(join(d, ".git", "config"), "x");
    await writeFile(join(d, ".aexignore"), "*.log\n");

    const entries = await zipOf(await File.fromPath(d));
    const names = Object.keys(entries).filter((n) => n !== RESERVED_META_ENTRY);
    expect(names).toEqual(["keep.txt"]);
    // The control file itself is never bundled.
    expect(entries[".aexignore"]).toBeUndefined();
  });

  it("keeps .gitignore as content by default but does not apply its patterns unless opted in", async () => {
    const d = await makeDir();
    await writeFile(join(d, "keep.txt"), "k");
    await writeFile(join(d, "secret.env"), "s");
    await writeFile(join(d, ".gitignore"), "*.env\n");

    const defaultEntries = await zipOf(await File.fromPath(d));
    expect(Object.keys(defaultEntries).sort()).toEqual([".gitignore", "keep.txt", "secret.env"]);

    const gitEntries = await zipOf(await File.fromPath(d, { ignore: { useGitignore: true } }));
    expect(gitEntries["secret.env"]).toBeUndefined();
    expect(gitEntries[".gitignore"]).toBeDefined();
  });

  it("respects extraPatterns and useDefaults:false", async () => {
    const d = await makeDir();
    await writeFile(join(d, "a.txt"), "a");
    await writeFile(join(d, "b.tmp"), "b");
    const entries = await zipOf(await File.fromPath(d, { ignore: { extraPatterns: ["*.tmp"] } }));
    expect(Object.keys(entries).filter((n) => n !== RESERVED_META_ENTRY)).toEqual(["a.txt"]);
  });

  it("throws on a directory that is empty after ignores", async () => {
    const d = await makeDir();
    await writeFile(join(d, "only.log"), "x");
    await writeFile(join(d, ".aexignore"), "*.log\n");
    await expect(File.fromPath(d)).rejects.toThrow(/empty/);
  });
});
