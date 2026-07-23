import { describe, expect, it } from "bun:test";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { CANONICAL_SHA256_DIGEST_PATTERN } from "@aexhq/contracts";
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
    expect(CANONICAL_SHA256_DIGEST_PATTERN.test(bundle.contentHash)).toBe(true);
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
    ).rejects.toThrow(/entry "index\.js" is not present in files/);
  });

  it("rejects a non-JS entry at authoring time (WS6 JS-module guard)", async () => {
    // A shell entry is rejected AT BUILD time, not mid-session by the tool executor.
    await expect(
      Tool.fromFiles({
        name: "calendar_lookup",
        description: "Non-JS entry.",
        inputSchema: { type: "object", properties: {}, required: [] },
        entry: "run.sh",
        files: { "run.sh": "#!/bin/sh\n" }
      })
    ).rejects.toThrow(/entry must be a JS module \(\.js\/\.mjs\/\.cjs\)/);

    // A matching .mjs entry present in files is accepted.
    const tool = await Tool.fromFiles({
      name: "calendar_lookup",
      description: "Good entry.",
      inputSchema: { type: "object", properties: {}, required: [] },
      entry: "index.mjs",
      files: { "index.mjs": "export default async function () {}\n" }
    });
    expect(tool.ref.entry).toBe("index.mjs");
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

describe("Tool submission", () => {
  it("requires the workspace publisher and exposes no alternate materialization API", async () => {
    const tool = await Tool.fromFiles({
      name: "calendar_lookup",
      description: "Looks up calendar availability.",
      inputSchema: { type: "object", properties: {}, required: [] },
      entry: "index.js",
      files: { "index.js": "export default async function () {}\n" }
    });
    expect(() => tool.toJSON()).toThrow(/publish with aex\.workspace\.tools\.publish/);
    expect("upload" in tool).toBe(false);
    expect("fromAsset" in Tool).toBe(false);
  });
});
