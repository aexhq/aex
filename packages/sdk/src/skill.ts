import {
  SKILL_NAME_PATTERN,
  type AssetRef,
  type FetchLike,
  type SkillRef
} from "@aexhq/contracts";
import { bundleSkillFiles, hashSkillBundle, type SkillFiles } from "./bundle.js";
import { fetchSkillArchive } from "./fetch-archive.js";
import { readDirectoryAsFiles } from "./node-fs.js";

/**
 * One `Skill` class for skill bytes. `client.submit` materializes the bytes
 * as an uploaded asset before the run lands; the wire ref becomes
 * `kind:"asset"`.
 *
 * Build from an inline files map (`Skill.fromFiles`), a local directory
 * (`Skill.fromPath`), or a remote zip archive over a signed URL
 * (`Skill.fromUrl`). All three converge on the same canonical bundle, so
 * identical content dedups across sources.
 *
 * Asset deduplication makes the same bytes a no-op upload on subsequent runs.
 * There is no `Skill.fromId(...)`. A URL is an ingestion source, not a
 * persistent reference.
 *
 * An inline draft is auto-staged to the content-addressable asset store at
 * submit time (the bytes upload before `POST /runs`; the wire ref becomes
 * `kind:"asset"`). Call `await skill.upload(client)` to pre-stage the bytes
 * explicitly — useful when you want to reuse the resulting `kind:"asset"`
 * Skill across multiple runs.
 */
export class Skill {
  readonly #ref: AssetRef | DraftSkillRef;
  readonly #inlineBytes: Uint8Array | undefined;
  #consumed = false;

  /**
   * Internal constructor. Use `Skill.fromFiles` or `Skill.fromPath` to create
   * instances.
   */
  private constructor(ref: AssetRef | DraftSkillRef, inlineBytes?: Uint8Array) {
    this.#ref = ref;
    this.#inlineBytes = inlineBytes;
  }

  /**
   * The wire-level reference. Returns the SDK-private draft shape for
   * un-materialized skills (kind:"draft", with name + contentHash).
   * `client.submit` walks these and uploads them before the run
   * lands.
   */
  get ref(): AssetRef | DraftSkillRef {
    return this.#ref;
  }

  /** True for local-bytes Skills that haven't been uploaded yet. */
  get isDraft(): boolean {
    return this.#ref.kind === "draft" && !this.#consumed;
  }

  get isConsumed(): boolean {
    return this.#consumed;
  }

  /**
   * Build a draft Skill from an inline files map. The SDK validates
   * basic safety (no path traversal, size caps, has `SKILL.md`),
   * deterministically zips the bundle, and computes the
   * `sha256:<hex>` content hash. `client.submit` materializes
   * these before the run lands.
   */
  static async fromFiles(args: { readonly name: string; readonly files: SkillFiles }): Promise<Skill> {
    if (!args || typeof args !== "object") {
      throw new Error("Skill.fromFiles: args is required");
    }
    if (typeof args.name !== "string" || !SKILL_NAME_PATTERN.test(args.name)) {
      throw new Error(`Skill.fromFiles: name must match ${SKILL_NAME_PATTERN.source}`);
    }
    const bundled = bundleSkillFiles(args.files);
    const contentHash = await hashSkillBundle(bundled.zip);
    const ref: DraftSkillRef = {
      kind: "draft",
      name: args.name,
      contentHash
    };
    return new Skill(ref, bundled.zip);
  }

  /**
   * Read a local directory and build a draft Skill. Symlinks and
   * non-regular files are skipped. Node-only.
   */
  static async fromPath(rootDir: string, args: { readonly name: string }): Promise<Skill> {
    const files = await readDirectoryAsFiles(rootDir);
    return Skill.fromFiles({ name: args.name, files });
  }

  /**
   * Fetch a zip-archived skill from a URL and build a draft Skill. The archive
   * is downloaded in the SDK process, so the URL is caller-controlled — host
   * the skill yourself and pass a temporary signed URL (e.g. an S3 presigned
   * URL). Its bytes are optionally integrity-checked against `sha256`, unzipped,
   * and reduced to the same files map as `Skill.fromFiles` — so a URL-sourced
   * skill and the identical local skill produce the same canonical asset and
   * dedup against each other.
   *
   * The archive must contain `SKILL.md` at its root, or inside a single
   * top-level folder (which is stripped). The signed URL only needs to be valid
   * for this call; `client.submit` snapshots the bytes into the run.
   *
   * Universal (Node 18+ / browser): requires a global `fetch`, or pass one.
   */
  static async fromUrl(
    url: string,
    args: {
      readonly name: string;
      readonly sha256?: string;
      readonly timeoutMs?: number;
      readonly fetch?: FetchLike;
    }
  ): Promise<Skill> {
    if (!args || typeof args !== "object") {
      throw new Error("Skill.fromUrl: args is required");
    }
    if (typeof args.name !== "string" || !SKILL_NAME_PATTERN.test(args.name)) {
      throw new Error(`Skill.fromUrl: name must match ${SKILL_NAME_PATTERN.source}`);
    }
    const files = await fetchSkillArchive(url, {
      ...(args.sha256 !== undefined ? { sha256: args.sha256 } : {}),
      ...(args.timeoutMs !== undefined ? { timeoutMs: args.timeoutMs } : {}),
      ...(args.fetch !== undefined ? { fetch: args.fetch } : {})
    });
    return Skill.fromFiles({ name: args.name, files });
  }

  /**
   * Reference a skill already uploaded to the workspace catalog
   * (`aex skills upload` / `operations.createSkillBundle`) in a run.
   *
   * A catalog skill's bytes are a content-addressed asset, so referencing it
   * is just an `{ kind:"asset" }` ref — once a run snapshots the bytes, it is
   * the identical normalized flow as an inline or file-sourced skill. Pass the
   * `Skill` record returned by `client.skills.list()` / `.get()`:
   *
   *   const [s] = await client.skills.list();
   *   await client.submit({ ..., skills: [Skill.fromCatalog(s)] });
   *
   * The record must be `ready` (it has a content hash). Unlike the draft
   * builders this performs no upload — the bytes already live in the catalog.
   */
  static fromCatalog(record: { readonly name: string; readonly hash?: string | null }): Skill {
    if (!record || typeof record !== "object") {
      throw new Error("Skill.fromCatalog: a catalog skill record is required");
    }
    if (typeof record.name !== "string" || !SKILL_NAME_PATTERN.test(record.name)) {
      throw new Error(`Skill.fromCatalog: record.name must match ${SKILL_NAME_PATTERN.source}`);
    }
    const rawHash = typeof record.hash === "string" ? record.hash : "";
    const hashHex = rawHash.startsWith("sha256:") ? rawHash.slice("sha256:".length) : rawHash;
    if (!/^[0-9a-f]{64}$/.test(hashHex)) {
      throw new Error(
        "Skill.fromCatalog: record.hash must be a sha256 digest — only `ready` catalog skills are referenceable"
      );
    }
    const ref: AssetRef = { kind: "asset", assetId: `asset_${hashHex}`, name: record.name };
    return new Skill(ref);
  }

  /**
   * Internal: yield the draft's bytes + metadata so `client.submit`
   * can upload the asset. After this returns, the Skill is marked consumed
   * so a second submit call against the same instance throws
   * (avoid silently re-uploading; explicit re-construction is the
   * supported retry pattern).
   *
   * Returns undefined for already-materialized Skills.
   */
  _takeDraftBundle(): { name: string; contentHash: string; bytes: Uint8Array } | undefined {
    if (this.#consumed) {
      throw new Error(
        "Skill: cannot reuse a consumed Skill in submit. Build a fresh Skill via " +
          "Skill.fromPath(...) / Skill.fromFiles(...) per submit call."
      );
    }
    if (this.#ref.kind !== "draft" || !this.#inlineBytes) {
      return undefined;
    }
    this.#consumed = true;
    return {
      name: this.#ref.name,
      contentHash: this.#ref.contentHash,
      bytes: this.#inlineBytes
    };
  }

  /**
   * Pre-upload a draft Skill's bytes to the workspace asset store and return a
   * NEW materialized Skill carrying a `kind:"asset"` ref. Blocking: the upload
   * completes before this resolves. Submitting the returned Skill sends a plain
   * asset ref and the run pulls the bytes from storage.
   *
   * Consumes this draft (a draft becomes an asset exactly once); call only on a
   * draft built via `Skill.fromFiles` / `Skill.fromPath` / `Skill.fromUrl`.
   */
  async upload(client: SkillUploader): Promise<Skill> {
    const bundle = this._takeDraftBundle();
    if (!bundle) {
      throw new Error(
        "Skill.upload: only draft Skills can be uploaded. A Skill from " +
          "Skill.fromCatalog(...) is already materialized."
      );
    }
    const uploaded = await client._uploadAsset({
      bytes: bundle.bytes,
      hash: bundle.contentHash,
      contentType: "application/zip"
    });
    const ref: AssetRef = { kind: "asset", assetId: uploaded.assetId, name: bundle.name };
    return new Skill(ref);
  }

  toJSON(): SkillRef {
    if (this.#ref.kind === "draft") {
      throw new Error(
        "Skill: draft Skills cannot be JSON-serialised — they only become wire refs when " +
        "client.submit uploads the bytes as an asset."
      );
    }
    return this.#ref;
  }
}

/**
 * SDK-internal draft skill marker. Never reaches the wire; the
 * materialize step inside `client.submit` converts these to
 * `kind:"asset"` refs.
 */
export interface DraftSkillRef {
  readonly kind: "draft";
  readonly name: string;
  readonly contentHash: string;
}

/**
 * Minimal client surface `Skill.upload` needs. `AgentExecutor` satisfies it via
 * its internal `_uploadAsset`; defined structurally here so `skill.ts` does not
 * import `client.ts` (which would be circular — `client.ts` imports `Skill`).
 */
export interface SkillUploader {
  _uploadAsset(args: {
    readonly bytes: Uint8Array;
    readonly hash: string;
    readonly contentType?: string;
  }): Promise<{ readonly assetId: string }>;
}
