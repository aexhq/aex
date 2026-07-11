import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

const SCRIPT = String.raw`
import { deepStrictEqual, match, ok, strictEqual } from "node:assert/strict";
import { Aex, BuiltinTools, Skill, Tool } from "@aexhq/sdk";

const calls = [];
const stored = new Set();
let version = 0;
const decodeBody = async (body) => {
  if (typeof body === "string") return JSON.parse(body);
  return body;
};
const fetch = async (input, init = {}) => {
  const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
  const method = String(init.method ?? "GET").toUpperCase();
  const body = await decodeBody(init.body);
  calls.push({ url, method, body });
  if (url.endsWith("/api/assets/presign")) {
    const hex = body.hash.slice("sha256:".length);
    const exists = stored.has(hex);
    if (!exists) stored.add(hex);
    return Response.json(exists
      ? { exists: true, assetId: "asset_" + hex, contentHash: body.hash, sizeBytes: body.sizeBytes }
      : { exists: false, uploadUrl: "https://objects.example/" + hex, requiredHeaders: {} });
  }
  if (url.startsWith("https://objects.example/")) return new Response("", { status: 200 });
  if (url.endsWith("/api/assets/finalize")) {
    const hex = body.hash.slice("sha256:".length);
    return Response.json({ assetId: "asset_" + hex, contentHash: body.hash, sizeBytes: body.sizeBytes });
  }
  const resource = /^\/api\/workspace\/(skills|tools)$/.exec(new URL(url).pathname);
  if (resource && method === "POST") {
    version += 1;
    const kind = resource[1] === "skills" ? "skill" : "tool";
    return Response.json({ resource: {
      ...body,
      kind,
      resourceId: "wres_" + String(version).repeat(32),
      version,
      createdAt: "2026-07-10T00:00:00.000Z"
    }});
  }
  if (url.endsWith("/api/sessions") && method === "POST") {
    return Response.json({ session: { id: "session-1", status: "idle", acceptsMessages: true } }, { status: 201 });
  }
  return Response.json({ error: "unexpected", url, method }, { status: 404 });
};

const client = new Aex({ apiKey: "aex_test", baseUrl: "https://api.example", fetch });
const skill = await Skill.fromContent(
  "---\nname: report-skill\ndescription: Produce reports.\n---\n# Report\n",
  { name: "report-skill" }
);
const tool = await Tool.fromFiles({
  name: "lookup",
  description: "Look up a value.",
  inputSchema: { type: "object", properties: {}, additionalProperties: false },
  entry: "index.js",
  files: { "index.js": "export default async () => ({ content: [] });\n" }
});

for (const draft of [skill, tool]) {
  let message = "";
  try { JSON.stringify(draft); } catch (error) { message = error.message; }
  match(message, /publish|draft/i);
}

const skillRef = await client.workspace.skills.publish(skill);
const toolRef = await client.workspace.tools.publish(tool);
strictEqual(skillRef.kind, "skill");
strictEqual(toolRef.kind, "tool");
match(skillRef.resourceId, /^wres_[0-9a-f]{32}$/);
match(toolRef.resourceId, /^wres_[0-9a-f]{32}$/);
strictEqual(skillRef.assetId, "asset_" + skillRef.contentHash.slice("sha256:".length));
strictEqual(toolRef.assetId, "asset_" + toolRef.contentHash.slice("sha256:".length));

await client.sessions.create({
  model: "claude-haiku-4-5",
  assets: { skills: [skillRef], tools: [toolRef] },
  builtinTools: [BuiltinTools.grep],
  apiKeys: { anthropic: "sk-ant" }
});
const create = calls.find((call) => call.url.endsWith("/api/sessions"));
deepStrictEqual(create.body.submission.assets, {
  files: [],
  skills: [skillRef],
  tools: [toolRef],
  instructions: []
});
deepStrictEqual(create.body.submission.builtinTools, ["grep"]);
ok(!("skills" in create.body.submission));
ok(!("tools" in create.body.submission));

// Publishing equivalent bytes checks the content-addressed store before creating
// a new logical resource version and therefore performs only one object PUT.
const sameSkill = await Skill.fromContent(
  "---\nname: report-skill\ndescription: Produce reports.\n---\n# Report\n",
  { name: "report-skill" }
);
const second = await client.workspace.skills.publish(sameSkill);
strictEqual(second.assetId, skillRef.assetId);
const puts = calls.filter((call) => call.url.startsWith("https://objects.example/") && call.method === "PUT");
strictEqual(puts.length, 2); // one skill object and one tool object
deepStrictEqual(
  calls
    .filter((call) => call.method === "POST" && /^\/api\/workspace\/(skills|tools)$/.test(new URL(call.url).pathname))
    .map((call) => new URL(call.url).pathname),
  ["/api/workspace/skills", "/api/workspace/tools", "/api/workspace/skills"]
);

process.stdout.write(JSON.stringify({
  skillResourceId: skillRef.resourceId,
  toolResourceId: toolRef.resourceId,
  secondSkillVersion: second.version,
  objectPuts: puts.length
}));
`;

describe("installed SDK workspace skill/tool publication", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex();
  }, 240_000);

  afterAll(() => install?.cleanup());

  it("publishes immutable refs and submits only grouped assets", async () => {
    const path = join(install.installDir, "workspace-skill-tool.mjs");
    writeFileSync(path, SCRIPT);
    const child = await runCommand(getBunCommand(), [path], { cwd: install.installDir, timeoutMs: 120_000 });
    if (child.exitCode !== 0) {
      throw new Error(`workspace-skill-tool.mjs exited ${child.exitCode}\n${child.stderr}`);
    }
    const result = JSON.parse(child.stdout) as Record<string, unknown>;
    expect(result).toMatchObject({ secondSkillVersion: 3, objectPuts: 2 });
    expect(result.skillResourceId).toMatch(/^wres_[0-9a-f]{32}$/);
    expect(result.toolResourceId).toMatch(/^wres_[0-9a-f]{32}$/);
  });
});
