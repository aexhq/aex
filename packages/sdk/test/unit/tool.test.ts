import { describe, expect, it, vi } from "vitest";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Tool } from "../../src/tool.js";

function takeBundle(tool: Tool): { ref: any; contentHash: string; bytes: Uint8Array } {
  return (tool as unknown as {
    _takeDraftBundle(): { ref: any; contentHash: string; bytes: Uint8Array };
  })._takeDraftBundle();
}

describe("Tool.fromFiles", () => {
  it("builds a draft tool with provider-visible manifest metadata", async () => {
    const tool = await Tool.fromFiles({
      name: "calendar_lookup",
      description: "Looks up calendar availability.",
      inputSchema: {
        type: "object",
        properties: { start: { type: "string" } },
        required: ["start"]
      },
      entry: "index.js",
      files: {
        "index.js": "export default async function ({ input }) { return { content: [{ type: 'text', text: input.start }] }; }\n"
      }
    });

    expect(tool.isDraft).toBe(true);
    const bundle = takeBundle(tool);
    expect(bundle.contentHash).toMatch(/^sha256:[0-9a-f]{64}$/);
    expect(bundle.ref).toMatchObject({
      kind: "asset",
      name: "calendar_lookup",
      description: "Looks up calendar availability.",
      entry: "index.js"
    });
    expect(bundle.ref.input_schema).toEqual({
      type: "object",
      properties: { start: { type: "string" } },
      required: ["start"]
    });
  });

  it("rejects MCP-routed names and missing entry files", async () => {
    await expect(
      Tool.fromFiles({
        name: "calendar__lookup",
        description: "Bad name.",
        inputSchema: { type: "object", properties: {}, required: [] },
        entry: "index.js",
        files: { "index.js": "export default async function () {}\n" }
      })
    ).rejects.toThrow(/must not contain "__"/);

    await expect(
      Tool.fromFiles({
        name: "calendar_lookup",
        description: "Missing entry.",
        inputSchema: { type: "object", properties: {}, required: [] },
        entry: "index.js",
        files: { "other.js": "export default async function () {}\n" }
      })
    ).rejects.toThrow(/entry "index\.js" must exist/);
  });
});

describe("Tool.fromPath", () => {
  it("reads tool.json from a local directory", async () => {
    const root = await mkdtemp(join(tmpdir(), "aex-tool-"));
    await mkdir(join(root, "src"), { recursive: true });
    await writeFile(
      join(root, "tool.json"),
      JSON.stringify({
        name: "calendar_lookup",
        description: "Looks up calendar availability.",
        input_schema: { type: "object", properties: {}, required: [] },
        entry: "src/index.js"
      }),
      "utf8"
    );
    await writeFile(join(root, "src", "index.js"), "export default async function () {}\n", "utf8");

    const tool = await Tool.fromPath(root);
    expect(tool.ref.kind).toBe("draft");
    expect(tool.ref.name).toBe("calendar_lookup");
    expect(tool.ref.entry).toBe("src/index.js");
  });
});

describe("Tool.upload", () => {
  it("uploads a draft and returns a materialized asset-ref Tool", async () => {
    const tool = await Tool.fromFiles({
      name: "calendar_lookup",
      description: "Looks up calendar availability.",
      inputSchema: { type: "object", properties: {}, required: [] },
      entry: "index.js",
      files: { "index.js": "export default async function () {}\n" }
    });
    const client = {
      _uploadAsset: vi.fn(async (args: { hash: string }) => ({ assetId: `asset_${args.hash.slice("sha256:".length)}` }))
    };

    const uploaded = await tool.upload(client);
    expect(client._uploadAsset).toHaveBeenCalledTimes(1);
    expect(uploaded.toJSON()).toMatchObject({
      kind: "asset",
      name: "calendar_lookup",
      description: "Looks up calendar availability.",
      entry: "index.js"
    });
    // The draft is reusable (no consume): the original stays a draft and can be
    // uploaded again (uploads are content-hash deduped).
    expect(tool.isDraft).toBe(true);
    await expect(tool.upload(client)).resolves.toBeInstanceOf(Tool);
  });
});
