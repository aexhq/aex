import { describe, expect, it } from "bun:test";
import { resolve as resolvePath } from "node:path";
import { unzipSync } from "fflate";
import { executeCli } from "../src/main.js";
import { makeIo, type FetchCall } from "./support.js";

const CWD = "/tmp/cli-test";
const abs = (p: string): string => resolvePath(CWD, p);
const COMMON = ["--api-key", "tok-1", "--aex-url", "https://dash.example/"];

const SKILL_MD = "---\nname: report-skill\ndescription: A test skill for attach\n---\n# Report skill\n";

function attachFetch(call: FetchCall): Response {
  const url = new URL(call.url);
  if (url.pathname === "/api/assets/presign") {
    const body = call.body as { hash: string };
    return new Response(JSON.stringify({
      exists: false,
      uploadUrl: `https://object-storage.example.test/assets/${body.hash.slice("sha256:".length)}`,
      requiredHeaders: {}
    }), { status: 200, headers: { "content-type": "application/json" } });
  }
  if (url.hostname === "object-storage.example.test") {
    return new Response("", { status: 200 });
  }
  if (url.pathname === "/api/assets/finalize") {
    const body = call.body as { hash: string; sizeBytes: number };
    const hex = body.hash.slice("sha256:".length);
    return new Response(JSON.stringify({
      assetId: `asset_${hex}`,
      contentHash: body.hash,
      sizeBytes: body.sizeBytes
    }), { status: 200, headers: { "content-type": "application/json" } });
  }
  const workspaceMatch = /^\/api\/workspace\/(files|skills|tools|instructions)$/.exec(url.pathname);
  if (workspaceMatch && call.init.method === "POST") {
    const kind = workspaceMatch[1]!;
    const body = call.body as Record<string, unknown>;
    const suffix = { files: "1", skills: "2", tools: "3", instructions: "4" }[kind]!;
    const singular = { files: "file", skills: "skill", tools: "tool", instructions: "instruction" }[kind]!;
    return new Response(JSON.stringify({
      resource: {
        ...body,
        kind: singular,
        resourceId: `wres_${suffix.repeat(32)}`,
        version: 1,
        createdAt: "2026-07-10T00:00:00.000Z"
      }
    }), {
      status: 200,
      headers: { "content-type": "application/json" }
    });
  }
  if (url.pathname === "/api/sessions" && call.init.method === "POST") {
    return new Response(JSON.stringify({ session: { id: "s1", status: "idle", acceptsMessages: true, provider: "anthropic", runtimeSize: "0.25cpu-1gb" } }), {
      status: 201,
      headers: { "content-type": "application/json" }
    });
  }
  if (url.pathname === "/api/sessions/s1/messages" && call.init.method === "POST") {
    return new Response(JSON.stringify({
      session: { id: "s1", status: "running", acceptsMessages: false, provider: "anthropic", runtimeSize: "0.25cpu-1gb" },
      run: { sessionId: "s1", runId: "run-1", turnSeq: 1, phase: "running" },
      eventCursor: 1
    }), {
      status: 202,
      headers: { "content-type": "application/json" }
    });
  }
  return new Response(JSON.stringify({ error: "unexpected", path: url.pathname }), {
    status: 404,
    headers: { "content-type": "application/json" }
  });
}

describe("aex start workspace resource flags", () => {
  it("preserves arbitrary bytes supplied through --file", async () => {
    const original = new Uint8Array([0, 0xff, 0xfe, 0x80, 0x41, 0xc3, 0x28]);
    let uploaded: Uint8Array | undefined;
    const cap = makeIo({
      argv: [
        "start",
        "--model", "anthropic/claude-haiku-4-5",
        "--prompt", "hi",
        "--file", "@binary.dat",
        ...COMMON
      ],
      binaryFiles: { [abs("binary.dat")]: original },
      fetchHandler: (call) => {
        if (new URL(call.url).hostname === "object-storage.example.test") {
          uploaded = call.init.body as Uint8Array;
        }
        return attachFetch(call);
      }
    });

    await executeCli(cap.io);

    expect(cap.exitCode).toBe(0);
    expect(uploaded).toBeDefined();
    expect(unzipSync(uploaded!)["binary.dat"]).toEqual(original);
  });

  it("publishes every attached resource and submits immutable refs", async () => {
    const cap = makeIo({
      argv: [
        "start",
        "--model", "anthropic/claude-haiku-4-5",
        "--prompt", "hi",
        "--skill", "@s.md",
        "--tool", "@t.js",
        "--instructions", "@a.md",
        "--file", "@f.txt",
        ...COMMON
      ],
      files: {
        [abs("s.md")]: SKILL_MD,
        [abs("t.js")]: "export default async () => ({ ok: true });\n",
        [abs("a.md")]: "# Agent brief\nBe concise.\n",
        [abs("f.txt")]: "reference data\n"
      },
      fetchHandler: attachFetch
    });

    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls.filter((call) => new URL(call.url).pathname === "/api/assets/presign")).toHaveLength(4);
    expect(cap.calls.filter((call) => new URL(call.url).hostname === "object-storage.example.test")).toHaveLength(4);
    expect(cap.calls.filter((call) => new URL(call.url).pathname === "/api/assets/finalize")).toHaveLength(4);
    for (const kind of ["files", "skills", "tools", "instructions"]) {
      expect(cap.calls.some((call) => new URL(call.url).pathname === `/api/workspace/${kind}`)).toBe(true);
    }

    const create = cap.calls.find((call) => new URL(call.url).pathname === "/api/sessions");
    expect(create).toBeDefined();
    const body = create!.body as Record<string, unknown>;
    expect("input" in body).toBe(false);
    const message = cap.calls.find((call) => new URL(call.url).pathname === "/api/sessions/s1/messages");
    expect(message?.body).toEqual({ input: ["hi"] });
    const submission = body.submission as {
      assets: {
        skills: Array<{ kind: string; resourceId: string; version: number }>;
        tools: Array<{ kind: string; resourceId: string; version: number }>;
        instructions: Array<{ kind: string; resourceId: string; version: number }>;
        files: Array<{ kind: string; resourceId: string; version: number }>;
      };
    };
    expect(submission.assets.skills[0]).toMatchObject({ kind: "skill", resourceId: `wres_${"2".repeat(32)}`, version: 1 });
    expect(submission.assets.tools[0]).toMatchObject({ kind: "tool", resourceId: `wres_${"3".repeat(32)}`, version: 1 });
    expect(submission.assets.instructions[0]).toMatchObject({ kind: "instruction", resourceId: `wres_${"4".repeat(32)}`, version: 1 });
    expect(submission.assets.files[0]).toMatchObject({ kind: "file", resourceId: `wres_${"1".repeat(32)}`, version: 1 });
  });

  it("reports a clear error when an attached asset file is missing", async () => {
    const cap = makeIo({
      argv: [
        "start",
        "--model", "anthropic/claude-haiku-4-5",
        "--prompt", "hi",
        "--file", "@missing.txt",
        ...COMMON
      ]
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("failed to attach asset");
    expect(cap.calls).toHaveLength(0);
  });
});
