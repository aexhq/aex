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
  type SkillFiles,
  type ToolBundleManifest
} from "./bundle.js";
import { readDirectoryAsFiles } from "./node-fs.js";

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

  static async fromFiles(args: ToolManifestInput & { readonly files: SkillFiles }): Promise<Tool> {
    if (!args || typeof args !== "object") {
      throw new Error("Tool.fromFiles: args is required");
    }
    const manifest = normalizeToolManifest("Tool.fromFiles", args);
    const bundled = bundleToolFiles(args.files, manifest);
    const contentHash = await hashSkillBundle(bundled.zip);
    const ref: DraftToolRef = {
      kind: "draft",
      assetId: `asset_${contentHash.slice("sha256:".length)}`,
      contentHash,
      ...manifest
    };
    return new Tool(ref, bundled.zip);
  }

  static async fromPath(rootDir: string): Promise<Tool> {
    const files = await readDirectoryAsFiles(rootDir);
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
      files: bundleFiles
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
        "Tool: draft Tools cannot be JSON-serialised — they only become wire refs when aex.run / openSession uploads the bytes as an asset."
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

function normalizeToolManifest(source: string, input: ToolManifestInput): ToolBundleManifest {
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
  return {
    name,
    description,
    input_schema: inputSchema,
    entry
  };
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
