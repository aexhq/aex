import { describe, expect, it, vi } from "vitest";
import { OutputsClient, SessionClient } from "@aexhq/sdk";
import { runCli } from "../src/run.js";
import { makeIo } from "./support.js";

const COMMON = ["--api-key", "tok-1", "--aex-url", "https://dash.example/"];

/** Stub the per-session outputs accessor `aex.sessions.outputs(id)` returns. */
function stubSessionOutputs() {
  const read = vi.fn().mockResolvedValue({ output: { id: "o1", filename: "data.json" }, text: "{\"ok\":1}", truncated: false, totalBytes: 8 });
  const download = vi.fn().mockResolvedValue(new TextEncoder().encode("{\"ok\":1}"));
  const link = vi.fn().mockResolvedValue({ url: "https://signed.example/o1", expiresInSeconds: 3600 });
  const find = vi.fn().mockResolvedValue([{ id: "o1", filename: "data.json" }]);
  const list = vi.fn().mockResolvedValue([{ id: "o1", filename: "data.json" }]);
  vi.spyOn(SessionClient.prototype, "outputs").mockReturnValue({ read, download, link, find, list } as never);
  return { read, download, link, find, list };
}

describe("aex outputs single-file sub-verbs (T6b)", () => {
  it("`outputs read <id> <path>` routes to the accessor.read and prints the text", async () => {
    const { read } = stubSessionOutputs();
    const cap = makeIo({ argv: ["outputs", "read", "s1", "data.json", ...COMMON] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(read).toHaveBeenCalledWith({ path: "data.json" });
    expect(JSON.parse(cap.stdout.trim())).toMatchObject({ text: "{\"ok\":1}", truncated: false });
  });

  it("`outputs download <id> <path> --out` routes to accessor.download and writes the bytes", async () => {
    const { download } = stubSessionOutputs();
    const writes = new Map<string, Uint8Array>();
    const cap = makeIo({ argv: ["outputs", "download", "s1", "data.json", "--out", "out.json", ...COMMON], writes });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(download).toHaveBeenCalledWith({ path: "data.json" });
    expect([...writes.keys()][0]).toMatch(/out\.json$/);
    expect(new TextDecoder().decode([...writes.values()][0]!)).toBe("{\"ok\":1}");
  });

  it("`outputs link <id> <path>` routes to accessor.link and prints the URL", async () => {
    const { link } = stubSessionOutputs();
    const cap = makeIo({ argv: ["outputs", "link", "s1", "data.json", ...COMMON] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(link).toHaveBeenCalledWith({ path: "data.json" });
    expect(JSON.parse(cap.stdout.trim()).url).toBe("https://signed.example/o1");
  });

  it("`outputs find <id> --name` routes to accessor.find", async () => {
    const { find } = stubSessionOutputs();
    const cap = makeIo({ argv: ["outputs", "find", "s1", "--name", "data", ...COMMON] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(find).toHaveBeenCalledWith({ filename: "data" });
    expect(JSON.parse(cap.stdout.trim())).toMatchObject({ id: "o1" });
  });

  it("`outputs search --query` routes to the cross-run aex.outputs.search", async () => {
    const search = vi
      .spyOn(OutputsClient.prototype, "search")
      .mockResolvedValue({ hits: [{ runId: "r1", outputId: "o1", filename: "data.json" }] });
    const cap = makeIo({ argv: ["outputs", "search", "--query", "data", ...COMMON] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(search).toHaveBeenCalledWith({ filename: "data" });
    expect(JSON.parse(cap.stdout.trim()).hits).toHaveLength(1);
  });

  it("`outputs <id>` (bare) still lists via the accessor", async () => {
    const { list } = stubSessionOutputs();
    const cap = makeIo({ argv: ["outputs", "s1", ...COMMON] });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(list).toHaveBeenCalledTimes(1);
    expect(JSON.parse(cap.stdout.trim())).toMatchObject({ id: "o1" });
  });
});
