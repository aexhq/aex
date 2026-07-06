import { describe, expect, it } from "vitest";
import { resolve as resolvePath } from "node:path";
import { runCli } from "../src/run.js";
import { makeIo, type FetchCall } from "./support.js";

const CWD = "/tmp/cli-test";
const abs = (p: string): string => resolvePath(CWD, p);
const COMMON = ["--api-key", "tok-1", "--aex-url", "https://dash.example/"];

const SKILL_MD = "---\nname: report-skill\ndescription: A test skill for attach\n---\n# Report skill\n";

function attachFetch(call: FetchCall): Response {
  const url = new URL(call.url);
  if (url.pathname === "/assets/presign") {
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
  if (url.pathname === "/assets/finalize") {
    const body = call.body as { hash: string; sizeBytes: number };
    const hex = body.hash.slice("sha256:".length);
    return new Response(JSON.stringify({
      assetId: `asset_${hex}`,
      contentHash: body.hash,
      sizeBytes: body.sizeBytes
    }), { status: 200, headers: { "content-type": "application/json" } });
  }
  if (url.pathname.startsWith("/api/skills/") && call.init.method === "PUT") {
    const name = decodeURIComponent(url.pathname.slice("/api/skills/".length));
    return new Response(JSON.stringify({ skill: { name }, updated: true }), {
      status: 200,
      headers: { "content-type": "application/json" }
    });
  }
  if (url.pathname === "/api/sessions" && call.init.method === "POST") {
    return new Response(JSON.stringify({ id: "s1", status: "idle", provider: "anthropic", runtime: "managed" }), {
      status: 200,
      headers: { "content-type": "application/json" }
    });
  }
  if (url.pathname === "/api/sessions/s1/messages" && call.init.method === "POST") {
    return new Response(JSON.stringify({
      session: { id: "s1", status: "running", turnSeq: 1, turnStatus: "launching", provider: "anthropic", runtime: "managed" },
      turn: { sessionId: "s1", turnSeq: 1 },
      eventCursor: 1
    }), {
      status: 200,
      headers: { "content-type": "application/json" }
    });
  }
  return new Response(JSON.stringify({ error: "unexpected", path: url.pathname }), {
    status: 404,
    headers: { "content-type": "application/json" }
  });
}

describe("aex run --skill/--tool/--agents-md/--file (T6a attach)", () => {
  it("stages every attached asset kind and submits their public refs", async () => {
    const cap = makeIo({
      argv: [
        "run",
        "--model", "claude-haiku-4-5",
        "--prompt", "hi",
        "--skill", "@s.md",
        "--tool", "@t.js",
        "--agents-md", "@a.md",
        "--file", "@f.txt",
        "--anthropic-api-key", "sk-ant-1",
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

    await runCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls.filter((call) => new URL(call.url).pathname === "/assets/presign")).toHaveLength(4);
    expect(cap.calls.filter((call) => new URL(call.url).hostname === "object-storage.example.test")).toHaveLength(4);
    expect(cap.calls.filter((call) => new URL(call.url).pathname === "/assets/finalize")).toHaveLength(4);
    expect(cap.calls.some((call) => new URL(call.url).pathname === "/api/skills/report-skill")).toBe(true);

    const create = cap.calls.find((call) => new URL(call.url).pathname === "/api/sessions");
    expect(create).toBeDefined();
    const body = create!.body as Record<string, unknown>;
    expect("input" in body).toBe(false);
    const message = cap.calls.find((call) => new URL(call.url).pathname === "/api/sessions/s1/messages");
    expect(message?.body).toEqual({ input: ["hi"] });
    const submission = body.submission as {
      skills?: unknown[];
      tools?: Array<{ kind?: string }>;
      agentsMd?: unknown[];
      files?: unknown[];
    };
    expect(submission.skills).toEqual([{ kind: "skill", name: "report-skill" }]);
    expect(submission.tools?.some((tool) => tool.kind === "asset")).toBe(true);
    expect(submission.agentsMd).toHaveLength(1);
    expect(submission.files).toHaveLength(1);
  });

  it("reports a clear error when an attached asset file is missing", async () => {
    const cap = makeIo({
      argv: [
        "run",
        "--model", "claude-haiku-4-5",
        "--prompt", "hi",
        "--file", "@missing.txt",
        "--anthropic-api-key", "sk-ant-1",
        ...COMMON
      ]
    });
    await runCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("failed to attach asset");
    expect(cap.calls).toHaveLength(0);
  });
});
