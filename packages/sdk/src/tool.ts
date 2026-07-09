import {
  TOOL_NAME_PATTERN,
  normaliseSkillBundlePath,
  type AssetRef,
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
  readonly #ref: ToolRef | DraftToolRef;
  readonly #inlineBytes: Uint8Array | undefined;
  /** Asset id cached after the first use, so reuse skips a re-upload. */
  #assetId: string | undefined;

  private constructor(ref: ToolRef | DraftToolRef, inlineBytes?: Uint8Array) {
    this.#ref = ref;
    this.#inlineBytes = inlineBytes;
  }

  get ref(): ToolRef | DraftToolRef {
    return this.#ref;
  }

  get isDraft(): boolean {
    return this.#ref.kind === "draft";
  }

  /** Internal: the asset id resolved on a prior use, or undefined. */
  get _cachedAssetId(): string | undefined {
    return this.#assetId;
  }

  /** Internal: remember the asset id resolved for this draft's bytes. */
  _rememberAsset(assetId: string): void {
    this.#assetId = assetId;
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

  static fromAsset(ref: ToolRef): Tool {
    return new Tool(normalizeToolRef("Tool.fromAsset", ref));
  }

  _takeDraftBundle(): { ref: ToolRef; contentHash: string; bytes: Uint8Array } | undefined {
    if (this.#ref.kind !== "draft" || !this.#inlineBytes) {
      return undefined;
    }
    const { kind: _kind, contentHash, ...ref } = this.#ref;
    return {
      ref: { kind: "asset", ...ref },
      contentHash,
      bytes: this.#inlineBytes
    };
  }

  async upload(client: ToolUploader): Promise<Tool> {
    const bundle = this._takeDraftBundle();
    if (!bundle) {
      throw new Error("Tool.upload: only draft Tools can be uploaded. A Tool.fromAsset(...) is already materialized.");
    }
    const uploaded = await client._uploadAsset({
      bytes: bundle.bytes,
      hash: bundle.contentHash,
      contentType: "application/zip"
    });
    return new Tool({ ...bundle.ref, assetId: uploaded.assetId });
  }

  toJSON(): ToolRef {
    if (this.#ref.kind === "draft") {
      throw new Error(
        "Tool: draft Tools cannot be JSON-serialised — they only become wire refs when aex.start / openSession uploads the bytes as an asset."
      );
    }
    return this.#ref;
  }
}

export interface DraftToolRef extends Omit<ToolRef, "kind"> {
  readonly kind: "draft";
  readonly contentHash: string;
}

export interface ToolUploader {
  _uploadAsset(args: {
    readonly bytes: Uint8Array;
    readonly hash: string;
    readonly contentType?: string;
  }): Promise<{ readonly assetId: string }>;
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

function normalizeToolRef(source: string, ref: ToolRef): ToolRef {
  const manifest = normalizeToolManifest(source, {
    name: ref.name,
    description: ref.description,
    input_schema: ref.input_schema,
    entry: ref.entry
  });
  if (ref.kind !== "asset") {
    throw new Error(`${source}: ref.kind must be "asset"`);
  }
  if (typeof ref.assetId !== "string" || !/^asset_[A-Za-z0-9_-]{8,128}$/.test(ref.assetId)) {
    throw new Error(`${source}: ref.assetId must be an asset id`);
  }
  return { kind: "asset", assetId: ref.assetId, ...manifest };
}
