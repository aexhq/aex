import {
  TOOL_NAME_PATTERN,
  type FetchLike,
  type SkillToolRef
} from "@aexhq/contracts";
import { bundleSkillFiles, hashSkillBundle, type SkillFiles } from "./bundle.js";
import { fetchSkillArchive } from "./fetch-archive.js";
import { readDirectoryAsFiles } from "./node-fs.js";

/**
 * A skill re-expressed as a TOOL. `Tools.fromSkillDir` / `Tools.fromSkillUrl`
 * read a skill folder/zip, lift the tool `name` + `description` from the
 * `SKILL.md` YAML frontmatter, and canonically bundle+hash the bytes. The
 * result rides in the session's `tools` array (next to builtin names and custom
 * {@link Tool} bundles); `client.run` / `openSession` uploads the bundle as an
 * asset before the run lands, and the wire ref becomes a
 * `{ kind:"skill", assetId, name, description }` {@link SkillToolRef}.
 *
 * At run time the model calls the no-arg load-tool to pull the skill's
 * `SKILL.md` body into context; the bundle's files are eagerly staged to
 * `/workspace/skills/<name>/`.
 *
 * Asset deduplication makes the same bytes a no-op upload on subsequent runs.
 * A URL is an ingestion source, not a persistent reference.
 */
export class SkillTool {
  readonly #ref: SkillToolRef | DraftSkillToolRef;
  readonly #inlineBytes: Uint8Array | undefined;
  /** Asset id cached after the first use, so reuse skips a re-upload. */
  #assetId: string | undefined;

  /** Internal constructor. Use the `Tools.fromSkill*` factories. */
  private constructor(ref: SkillToolRef | DraftSkillToolRef, inlineBytes?: Uint8Array) {
    this.#ref = ref;
    this.#inlineBytes = inlineBytes;
  }

  /**
   * The wire-level reference. Returns the SDK-private draft shape for
   * un-uploaded skill-tools (kind:"draft", with name + description +
   * contentHash). `client.run` / `openSession` walks these and uploads them
   * before the run lands.
   */
  get ref(): SkillToolRef | DraftSkillToolRef {
    return this.#ref;
  }

  /** True for local-bytes skill-tools that haven't been uploaded yet. */
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

  /** Internal: build a draft from an already-loaded skill files map. */
  static async _fromFiles(
    source: string,
    files: SkillFiles,
    nameOverride: string | undefined
  ): Promise<SkillTool> {
    const front = extractSkillFrontmatter(source, files);
    const name = nameOverride ?? front.name;
    if (typeof name !== "string" || name.length === 0) {
      throw new Error(
        `${source}: a skill name is required — pass { name } or add a \`name:\` field to the SKILL.md YAML frontmatter`
      );
    }
    if (!TOOL_NAME_PATTERN.test(name)) {
      throw new Error(`${source}: name must match ${TOOL_NAME_PATTERN.source}`);
    }
    if (name.includes("__")) {
      throw new Error(`${source}: name must not contain "__"; that separator is reserved for MCP tools`);
    }
    const description = front.description;
    if (typeof description !== "string" || description.trim().length === 0) {
      throw new Error(
        `${source}: a skill description is required — add a \`description:\` field to the SKILL.md YAML frontmatter`
      );
    }
    if (description.length > 2048) {
      throw new Error(`${source}: description must be <= 2048 chars`);
    }
    const bundled = bundleSkillFiles(files);
    const contentHash = await hashSkillBundle(bundled.zip);
    const ref: DraftSkillToolRef = { kind: "draft", name, description, contentHash };
    return new SkillTool(ref, bundled.zip);
  }

  /**
   * Internal: yield the draft's bytes + metadata so `client.run` / `openSession`
   * can upload the asset. Idempotent (non-consuming): a SkillTool is reusable
   * across sessions — the first use caches the resolved asset id (see
   * `_rememberAsset`) so later uses reuse it instead of re-uploading.
   *
   * Returns undefined for already-uploaded skill-tools.
   */
  _takeDraftBundle(): { name: string; description: string; contentHash: string; bytes: Uint8Array } | undefined {
    if (this.#ref.kind !== "draft" || !this.#inlineBytes) {
      return undefined;
    }
    return {
      name: this.#ref.name,
      description: this.#ref.description,
      contentHash: this.#ref.contentHash,
      bytes: this.#inlineBytes
    };
  }

  toJSON(): SkillToolRef {
    if (this.#ref.kind === "draft") {
      throw new Error(
        "SkillTool: draft skill-tools cannot be JSON-serialised — they only become wire refs when " +
          "aex.run / openSession uploads the bytes as an asset."
      );
    }
    return this.#ref;
  }
}

/**
 * SDK-internal draft skill-tool marker. Never reaches the wire; the
 * materialize step inside `client.run` / `openSession` converts these to
 * `kind:"skill"` refs once the bundle is uploaded.
 */
export interface DraftSkillToolRef {
  readonly kind: "draft";
  readonly name: string;
  readonly description: string;
  readonly contentHash: string;
}

/**
 * Factory namespace for skill-tools. Each factory reads a skill bundle, lifts
 * `name` + `description` from the `SKILL.md` frontmatter (an explicit `name`
 * argument overrides the frontmatter), and produces a {@link SkillTool} to pass
 * in the session's `tools` array.
 */
export const Tools = {
  /**
   * Read a local skill directory and build a skill-tool. The directory must
   * contain `SKILL.md` at its root, whose YAML frontmatter provides the tool
   * `name` (unless overridden via `args.name`) and `description`. Symlinks and
   * non-regular files are skipped. Bun/Node filesystem runtimes only.
   */
  async fromSkillDir(rootDir: string, args: { readonly name?: string } = {}): Promise<SkillTool> {
    const files = await readDirectoryAsFiles(rootDir);
    return SkillTool._fromFiles("Tools.fromSkillDir", files, args.name);
  },

  /**
   * Fetch a zip-archived skill from a URL and build a skill-tool. The archive is
   * downloaded in the SDK process, so the URL is caller-controlled — host the
   * skill yourself and pass a temporary signed URL (e.g. an S3 presigned URL).
   * Its bytes are optionally integrity-checked against `sha256`, unzipped, and
   * reduced to the same files map as `Tools.fromSkillDir`, so a URL-sourced
   * skill and the identical local skill produce the same canonical asset and
   * dedup against each other.
   *
   * The archive must contain `SKILL.md` at its root, or inside a single
   * top-level folder (which is stripped). The signed URL only needs to be valid
   * for this call; `client.run` / `openSession` snapshots the bytes into the run.
   *
   * Universal (Bun / Node 18+ / browser): requires a global `fetch`, or pass one.
   */
  async fromSkillUrl(
    url: string,
    args: {
      readonly name?: string;
      readonly sha256?: string;
      readonly timeoutMs?: number;
      readonly fetch?: FetchLike;
    } = {}
  ): Promise<SkillTool> {
    const files = await fetchSkillArchive(url, {
      ...(args.sha256 !== undefined ? { sha256: args.sha256 } : {}),
      ...(args.timeoutMs !== undefined ? { timeoutMs: args.timeoutMs } : {}),
      ...(args.fetch !== undefined ? { fetch: args.fetch } : {})
    });
    return SkillTool._fromFiles("Tools.fromSkillUrl", files, args.name);
  }
} as const;

/**
 * Read `SKILL.md` from a bundle files map and parse its YAML frontmatter for
 * the `name` + `description` fields. Throws when the bundle has no root
 * `SKILL.md` (that is what makes a bundle a skill).
 */
function extractSkillFrontmatter(
  source: string,
  files: SkillFiles
): { name?: string; description?: string } {
  const raw = files["SKILL.md"];
  if (raw === undefined) {
    throw new Error(`${source}: the skill bundle must contain a SKILL.md at its root`);
  }
  const text = typeof raw === "string" ? raw : new TextDecoder().decode(raw);
  return parseSkillFrontmatter(text);
}

/**
 * Minimal YAML-frontmatter reader: pulls the `name` and `description` scalar
 * values out of the leading `--- … ---` block. Only simple single-line
 * `key: value` entries are supported (surrounding single/double quotes are
 * stripped); anything else is ignored. A skill with no frontmatter yields an
 * empty result and the caller reports the missing field.
 */
function parseSkillFrontmatter(text: string): { name?: string; description?: string } {
  const src = text.charCodeAt(0) === 0xfeff ? text.slice(1) : text;
  const match = /^---[ \t]*\r?\n([\s\S]*?)\r?\n---[ \t]*(?:\r?\n|$)/.exec(src);
  if (!match) {
    return {};
  }
  const out: { name?: string; description?: string } = {};
  for (const line of match[1]!.split(/\r?\n/)) {
    const kv = /^([A-Za-z0-9_-]+)[ \t]*:[ \t]*(.*)$/.exec(line);
    if (!kv) continue;
    const key = kv[1]!.toLowerCase();
    if (key !== "name" && key !== "description") continue;
    let value = kv[2]!.trim();
    if (
      value.length >= 2 &&
      ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'")))
    ) {
      value = value.slice(1, -1);
    }
    if (value.length > 0) {
      out[key] = value;
    }
  }
  return out;
}
