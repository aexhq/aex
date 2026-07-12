/**
 * SDK shape tests for the File primitive.
 *
 * A File wraps bytes (single file or zipped folder) in a canonical zip and is
 * UNZIPPED on the runtime into its `mountPath` DIRECTORY, preserving the real
 * filename + extension. These tests pin:
 *   - the single-entry zip carries the REAL filename (not a stripped slug);
 *   - `mountPath` defaults to `/workspace` and round-trips when set;
 *   - the storage slug (`ref.name`) is decoupled from the on-disk filename;
 *   - an ordinary filename (with `.`/`_`) is accepted by `fromBytes`.
 */
import { describe, expect, it } from "vitest";
import { mkdtemp, rm, writeFile, mkdir, truncate } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { unzipSync } from "fflate";
import { File } from "../../src/file.js";
import { ASSET_ARCHIVE_LIMITS, DEFAULT_FILE_MOUNT_PATH } from "@aexhq/contracts";

const TEXT = new TextEncoder();
const DEC = new TextDecoder();

/** Pull the draft's zipped bytes + ref so a test can inspect the zip + mountPath. */
function takeBundle(file: File): { name: string; mountPath: string; entries: Record<string, Uint8Array> } {
  const bundle = file._takeDraftBundle();
  if (!bundle) throw new Error("expected a draft bundle");
  return { name: bundle.name, mountPath: bundle.mountPath, entries: unzipSync(bundle.bytes) };
}

describe("File.fromBytes", () => {
  it("preserves the REAL filename (with extension) as the single zip entry", async () => {
    const file = await File.fromBytes({
      name: "source-video-subtitles.srt",
      bytes: TEXT.encode("1\n00:00:01 --> 00:00:02\nhi\n")
    });
    const { entries } = takeBundle(file);
    const names = Object.keys(entries);
    expect(names).toEqual(["source-video-subtitles.srt"]);
    expect(DEC.decode(entries["source-video-subtitles.srt"]!)).toContain("hi");
  });

  it("defaults mountPath to /workspace (the agent's cwd)", async () => {
    const file = await File.fromBytes({ name: "data.csv", bytes: TEXT.encode("a,b\n1,2\n") });
    const { mountPath } = takeBundle(file);
    expect(mountPath).toBe(DEFAULT_FILE_MOUNT_PATH);
    expect(mountPath).toBe("/workspace");
  });

  it("round-trips a custom absolute mountPath directory", async () => {
    const file = await File.fromBytes({
      name: "data.csv",
      bytes: TEXT.encode("x"),
      mountPath: "/workspace/input"
    });
    expect(takeBundle(file).mountPath).toBe("/workspace/input");
  });

  it("decouples the storage slug from the on-disk filename", async () => {
    // P3 fix: an ordinary filename (with `.` and `_`) is accepted; the slug is
    // derived internally and never strips the real name from the zip entry.
    const file = await File.fromBytes({
      name: "Source_Video.SRT",
      bytes: TEXT.encode("x")
    });
    const { name, entries } = takeBundle(file);
    expect(Object.keys(entries)).toEqual(["Source_Video.SRT"]); // real name preserved
    expect(name).toBe("source-video"); // slug: lowercased, ext dropped, kebab
  });

  it("rejects a filename containing a path separator", async () => {
    await expect(
      File.fromBytes({ name: "a/b.txt", bytes: TEXT.encode("x") })
    ).rejects.toThrow(/not a valid filename/);
  });

  it("rejects an out-of-workspace-shaped but malformed mountPath ('..' traversal)", async () => {
    await expect(
      File.fromBytes({ name: "a.txt", bytes: TEXT.encode("x"), mountPath: "/workspace/../etc" })
    ).rejects.toThrow(/traversal/);
  });

  it("rejects a relative mountPath", async () => {
    await expect(
      File.fromBytes({ name: "a.txt", bytes: TEXT.encode("x"), mountPath: "relative/dir" })
    ).rejects.toThrow(/absolute path/);
  });
});

describe("File.fromPath", () => {
  it("rejects an oversized sparse file before reading or framing it", async () => {
    const dir = await mkdtemp(join(tmpdir(), "aex-file-test-"));
    try {
      const p = join(dir, "oversized.bin");
      await writeFile(p, "x");
      await truncate(p, ASSET_ARCHIVE_LIMITS.maxDecompressedBytes + 1);
      await expect(File.fromPath(p)).rejects.toThrow(/128 MiB expanded limit/);
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  it("a single file preserves its real basename as the sole zip entry", async () => {
    const dir = await mkdtemp(join(tmpdir(), "aex-file-test-"));
    try {
      const p = join(dir, "subtitles.srt");
      await writeFile(p, "hello-srt");
      const file = await File.fromPath(p);
      const { mountPath, entries } = takeBundle(file);
      expect(mountPath).toBe("/workspace");
      expect(Object.keys(entries)).toEqual(["subtitles.srt"]);
      expect(DEC.decode(entries["subtitles.srt"]!)).toBe("hello-srt");
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  it("a directory preserves the real relative paths as zip entries", async () => {
    const dir = await mkdtemp(join(tmpdir(), "aex-file-test-"));
    try {
      await mkdir(join(dir, "src"), { recursive: true });
      await writeFile(join(dir, "README.md"), "# r");
      await writeFile(join(dir, "src", "a.ts"), "export const a = 1;");
      const file = await File.fromPath(dir, { mountPath: "/workspace/repo" });
      const { mountPath, entries } = takeBundle(file);
      expect(mountPath).toBe("/workspace/repo");
      expect(Object.keys(entries).sort()).toEqual(["README.md", "src/a.ts"]);
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
});
