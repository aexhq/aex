import {
  TOOL_NAME_PATTERN,
  normaliseSkillBundlePath,
  type ToolInputSchema,
  type ToolRef
} from "@aexhq/contracts";
import {
  bundleToolFiles,
  hashSkillBundle,
  type BundleMeta,
  type SkillFiles,
  type ToolBundleManifest
} from "./bundle.js";
import { readDirectoryWithFidelity } from "./node-fs.js";
import type { IgnoreOptions } from "./node-walk.js";

export interface ToolManifestInput {
  readonly name: string;
  readonly description: string;
  readonly inputSchema?: ToolInputSchema;
  readonly input_schema?: ToolInputSchema;
  readonly entry: string;
}

export class Tool {
  readonly #ref: DraftToolRef;
  readonly #inlineBytes: Uint8Array;

  private constructor(ref: DraftToolRef, inlineBytes: Uint8Array) {
    this.#ref = ref;
    this.#inlineBytes = inlineBytes;
  }

  get ref(): DraftToolRef {
    return this.#ref;
  }

  get isDraft(): boolean {
    return true;
  }

  static async fromFiles(
    args: ToolManifestInput & { readonly files: SkillFiles; readonly meta?: BundleMeta }
  ): Promise<Tool> {
    if (!args || typeof args !== "object") {
      throw new Error("Tool.fromFiles: args is required");
    }
    const manifest = normalizeToolManifest("Tool.fromFiles", args, args.files);
    const bundled = bundleToolFiles(args.files, manifest, args.meta);
    const contentHash = await hashSkillBundle(bundled.zip);
    const ref: DraftToolRef = {
      kind: "draft",
      assetId: `asset_${contentHash.slice("sha256:".length)}`,
      contentHash,
      ...manifest
    };
    return new Tool(ref, bundled.zip);
  }

  /**
   * Read a local tool bundle directory (`tool.json` at its root + support files).
   * The directory is walked with FIDELITY (the same walk `File.fromPath` uses):
   * `.aexignore` + the always-on defaults (`.git/`, `node_modules/`, …) prune the
   * upload, and executable bits + symlinks are captured into a `.aexmeta.json`
   * sidecar (emitted only when such metadata exists, so a pure-content tool dir
   * stays byte-identical → dedup continuity). Bun/Node filesystem runtimes only.
   */
  static async fromPath(rootDir: string, args?: { readonly ignore?: IgnoreOptions }): Promise<Tool> {
    const { files, meta } = await readDirectoryWithFidelity(rootDir, args?.ignore);
    const rawManifest = files["tool.json"];
    if (rawManifest === undefined) {
      throw new Error('Tool.fromPath: tool.json is required at the tool bundle root');
    }
    const text = typeof rawManifest === "string" ? rawManifest : new TextDecoder().decode(rawManifest);
    let parsed: unknown;
    try {
      parsed = JSON.parse(text);
    } catch (err) {
      throw new Error(`Tool.fromPath: tool.json is not valid JSON: ${(err as Error).message}`);
    }
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      throw new Error("Tool.fromPath: tool.json must be an object");
    }
    const { ["tool.json"]: _manifest, ...bundleFiles } = files;
    return Tool.fromFiles({
      ...(parsed as ToolManifestInput),
      files: bundleFiles,
      meta
    });
  }

  _takeDraftBundle(): { ref: ToolRef; contentHash: string; bytes: Uint8Array } {
    const { kind: _kind, contentHash, ...ref } = this.#ref;
    return {
      ref: { kind: "asset", ...ref },
      contentHash,
      bytes: this.#inlineBytes
    };
  }

  toJSON(): never {
    throw new Error("Tool drafts cannot be submitted directly; publish with aex.workspace.tools.publish(...)");
  }
}
export interface DraftToolRef extends Omit<ToolRef, "kind"> {
  readonly kind: "draft";
  readonly contentHash: string;
}


function normalizeToolManifest(source: string, input: ToolManifestInput, files?: SkillFiles): ToolBundleManifest {
  const name = input.name;
  if (typeof name !== "string" || !TOOL_NAME_PATTERN.test(name)) {
    throw new Error(`${source}: name must match ${TOOL_NAME_PATTERN.source}`);
  }
  if (name.includes("__")) {
    throw new Error(`${source}: name must not contain "__"; that separator is reserved for MCP tools`);
  }
  const description = input.description;
  if (typeof description !== "string" || description.trim().length === 0 || description.length > 2048) {
    throw new Error(`${source}: description must be non-empty and <= 2048 chars`);
  }
  const inputSchema = input.inputSchema ?? input.input_schema;
  if (!inputSchema || typeof inputSchema !== "object" || Array.isArray(inputSchema)) {
    throw new Error(`${source}: inputSchema must be a JSON Schema object`);
  }
  if ((inputSchema as { readonly type?: unknown }).type !== "object") {
    throw new Error(`${source}: inputSchema.type must be "object"`);
  }
  const entry = normaliseSkillBundlePath(input.entry);
  assertJsModuleEntry(source, entry, input.entry, files);
  return {
    name,
    description,
    input_schema: inputSchema,
    entry
  };
}

const JS_MODULE_ENTRY = /\.(?:js|mjs|cjs)$/i;

/**
 * Validate the tool's ENTRY is a JS module at authoring time (fail-fast), not
 * mid-session when the runtime module-loader rejects a `run.sh`. The entry must end
 * in `.js`/`.mjs`/`.cjs` and — when the bundle files are known
 * ({@link Tool.fromFiles}) — must be present in `files`.
 */
function assertJsModuleEntry(source: string, entry: string, rawEntry: string, files: SkillFiles | undefined): void {
  const basename = entry.split("/").pop() ?? entry;
  if (!JS_MODULE_ENTRY.test(basename)) {
    throw new Error(
      `${source}: entry must be a JS module (.js/.mjs/.cjs) that default-exports a function or { execute }; got ${JSON.stringify(rawEntry)}`
    );
  }
  if (files !== undefined && !(entry in files) && !(rawEntry in files)) {
    throw new Error(
      `${source}: entry ${JSON.stringify(rawEntry)} is not present in files (keys: ${Object.keys(files).join(", ") || "(none)"})`
    );
  }
}
