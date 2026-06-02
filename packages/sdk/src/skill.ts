import {
  SKILL_NAME_PATTERN,
  type ProviderSkillRef,
  type SkillRef
} from "@antpath/contracts";
import { bundleSkillFiles, hashSkillBundle, type SkillFiles } from "./bundle.js";
import { readDirectoryAsFiles } from "./node-fs.js";

/**
 * One `Skill` class, two usage modes:
 *
 *   - **Provider built-in** — references a provider-side skill (e.g.
 *     Anthropic's `pdf` / `xlsx` / `docx` / `pptx` prebuilt Agent
 *     Skills). Not uploaded.
 *     ```ts
 *     const pdf = Skill.provider({ vendor: "anthropic", skillId: "pdf" });
 *     ```
 *
 *   - **Local bytes** — built from local files. `client.submitRun`
 *     materializes the bytes to R2 (content-addressable, workspace-
 *     scoped) before the run lands; the wire ref becomes `kind:"r2"`.
 *     ```ts
 *     const rules = await Skill.fromFiles({ name: "rules", files: {...} });
 *     await client.submitRun({ skills: [rules], ... });
 *     ```
 *
 * The workspace pre-upload concept is gone — R2's content-addressable
 * dedup at submit time makes the same bytes a no-op upload on subsequent
 * runs. There is no `Skill.fromId(...)` and no `.upload(client)`.
 */
export class Skill {
  readonly #ref: SkillRef | DraftSkillRef;
  readonly #inlineBytes: Uint8Array | undefined;
  #consumed = false;

  /**
   * Internal constructor. Use `Skill.provider`, `Skill.fromFiles`, or
   * `Skill.fromPath` to create instances.
   */
  constructor(ref: SkillRef | DraftSkillRef, inlineBytes?: Uint8Array) {
    this.#ref = ref;
    this.#inlineBytes = inlineBytes;
  }

  /**
   * The wire-level reference. Returns the SDK-private draft shape for
   * un-materialized skills (kind:"draft", with name + contentHash).
   * `client.submitRun` walks these and uploads to R2 before the run
   * lands.
   */
  get ref(): SkillRef | DraftSkillRef {
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
   * Reference a provider built-in skill (e.g. Anthropic Skills).
   */
  static provider(args: {
    readonly vendor: ProviderSkillRef["vendor"];
    readonly skillId: string;
    readonly version?: string;
  }): Skill {
    if (!args || typeof args !== "object") {
      throw new Error("Skill.provider: args is required");
    }
    if (typeof args.vendor !== "string" || !args.vendor) {
      throw new Error("Skill.provider: vendor is required");
    }
    if (typeof args.skillId !== "string" || !args.skillId) {
      throw new Error("Skill.provider: skillId is required");
    }
    const ref: ProviderSkillRef = args.version
      ? { kind: "provider", vendor: args.vendor, skillId: args.skillId, version: args.version }
      : { kind: "provider", vendor: args.vendor, skillId: args.skillId };
    return new Skill(ref);
  }

  /**
   * Build a draft Skill from an inline files map. The SDK validates
   * basic safety (no path traversal, size caps, has `SKILL.md`),
   * deterministically zips the bundle, and computes the
   * `sha256:<hex>` content hash. `client.submitRun` materializes
   * these to R2 before the run lands.
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
   * Internal: yield the draft's bytes + metadata so `client.submitRun`
   * can upload to R2. After this returns, the Skill is marked consumed
   * so a second submitRun call against the same instance throws
   * (avoid silently re-uploading; explicit re-construction is the
   * supported retry pattern).
   *
   * Returns undefined for non-draft (provider / already-materialized)
   * Skills.
   */
  _takeDraftBundle(): { name: string; contentHash: string; bytes: Uint8Array } | undefined {
    if (this.#consumed) {
      throw new Error(
        "Skill: cannot reuse a consumed Skill in submitRun. Build a fresh Skill via " +
          "Skill.fromPath(...) / Skill.fromFiles(...) per submitRun call."
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

  toJSON(): SkillRef {
    if (this.#ref.kind === "draft") {
      throw new Error(
        "Skill: draft Skills cannot be JSON-serialised — they only become wire refs when " +
          "client.submitRun uploads the bytes to R2."
      );
    }
    return this.#ref;
  }
}

/**
 * SDK-internal draft skill marker. Never reaches the wire; the
 * materialize step inside `client.submitRun` converts these to
 * `kind:"r2"` refs.
 */
export interface DraftSkillRef {
  readonly kind: "draft";
  readonly name: string;
  readonly contentHash: string;
}
