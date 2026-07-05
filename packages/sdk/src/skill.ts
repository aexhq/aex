import {
  SKILL_NAME_PATTERN,
  SKILL_RESERVED_NAMES,
  type FetchLike,
  type SkillRef
} from "@aexhq/contracts";
import { bundleSkillFiles, hashSkillBundle, type SkillFiles } from "./bundle.js";
import { fetchSkillArchive } from "./fetch-archive.js";
import { readDirectoryAsFiles } from "./node-fs.js";
import { unzipSync } from "fflate";

/**
 * A Skill is a FIRST-CLASS, workspace-scoped, by-name bundle of instructional /
 * executable content (`SKILL.md` at the bundle root plus any supporting files).
 * It is DISTINCT from a {@link Tool}: skills are passed on the session's separate
 * `skills:` input, not `tools:`, and a run gets a single `skills` meta-tool
 * (list/load) rather than one load-tool per skill.
 *
 * Lifecycle mirrors `Secret` promotion, but keyed to a workspace name:
 *   - The `Skill.from*` factories read a bundle, lift `name` + `description` from
 *     the `SKILL.md` YAML frontmatter (an explicit `{ name }` overrides), and
 *     canonically zip + hash the bytes → a DRAFT skill.
 *   - `skill.upload(client)` UPSERTS the workspace skill by name: it stages the
 *     bytes to the content-addressed asset store (presign/finalize) and PUTs the
 *     registry entry. Identical bytes are a no-op. Returns an uploaded `Skill`
 *     whose wire ref is `{ kind:"skill", name }` (BY NAME — no assetId, no hash).
 *   - Passing a DRAFT skill in `skills:` auto-upserts it on submit (same
 *     ergonomic as a draft `Tool` / `File`).
 *
 * Binding is by name and mutable: a re-upload under the same name changes what
 * every future run referencing that name sees.
 */
export class Skill {
  #ref: SkillRef | DraftSkillRef;
  readonly #description: string;
  readonly #bundleBytes: Uint8Array | undefined;
  /** Set once this instance has upserted its bytes, so reuse skips the round-trip. */
  #uploadedName: string | undefined;

  /** Internal constructor. Use the `Skill.from*` factories. */
  private constructor(ref: SkillRef | DraftSkillRef, description: string, bundleBytes?: Uint8Array) {
    this.#ref = ref;
    this.#description = description;
    this.#bundleBytes = bundleBytes;
    if (ref.kind === "skill") {
      this.#uploadedName = ref.name;
    }
  }

  /** The wire ref: `{ kind:"skill", name }` once uploaded, or the draft shape before. */
  get ref(): SkillRef | DraftSkillRef {
    return this.#ref;
  }

  /** True for a local-bytes skill that has not been upserted to the workspace yet. */
  get isDraft(): boolean {
    return this.#ref.kind === "draft";
  }

  get name(): string {
    return this.#ref.name;
  }

  get description(): string {
    return this.#description;
  }

  /** Internal: the workspace name this instance already upserted, or undefined. */
  get _cachedName(): string | undefined {
    return this.#uploadedName;
  }

  /** Internal: remember that this instance's bytes were upserted under `name`. */
  _rememberUpload(name: string): void {
    this.#uploadedName = name;
  }

  // --- factories (source symmetry with File / AgentsMd / Tool) --------------

  /**
   * Read a local skill directory. It must contain `SKILL.md` at its root, whose
   * YAML frontmatter supplies `description` and (unless `args.name` is given)
   * `name`. When neither an explicit name nor a frontmatter name is present, the
   * slugified directory basename is used. Symlinks / non-regular files are
   * skipped. Bun/Node filesystem runtimes only.
   */
  static async fromDir(rootDir: string, args: { readonly name?: string } = {}): Promise<Skill> {
    const files = await readDirectoryAsFiles(rootDir);
    return Skill.#fromFiles("Skill.fromDir", files, args.name, dirBasename(rootDir));
  }

  /**
   * Fetch a zip-archived skill from a URL. The archive is downloaded in the SDK
   * process (caller-controlled URL — no SSRF surface), optionally integrity-checked
   * against `sha256`, and reduced to the same files map as `Skill.fromDir` (a
   * single wrapping top-level folder is stripped). It must expose `SKILL.md` at
   * the root. The signed URL only needs to be valid for this call.
   */
  static async fromUrl(
    url: string,
    args: {
      readonly name?: string;
      readonly sha256?: string;
      readonly timeoutMs?: number;
      readonly fetch?: FetchLike;
    } = {}
  ): Promise<Skill> {
    const files = await fetchSkillArchive(url, {
      ...(args.sha256 !== undefined ? { sha256: args.sha256 } : {}),
      ...(args.timeoutMs !== undefined ? { timeoutMs: args.timeoutMs } : {}),
      ...(args.fetch !== undefined ? { fetch: args.fetch } : {})
    });
    // A URL has no reliable directory basename, so no slug fallback.
    return Skill.#fromFiles("Skill.fromUrl", files, args.name, undefined);
  }

  /**
   * Build a draft skill from an in-memory files map (path -> string | bytes).
   * Requires a root `SKILL.md`. Universal (no filesystem access).
   */
  static async fromFiles(args: { readonly name?: string; readonly files: SkillFiles }): Promise<Skill> {
    if (!args || typeof args !== "object" || args.files === undefined) {
      throw new Error("Skill.fromFiles: { files } is required");
    }
    return Skill.#fromFiles("Skill.fromFiles", args.files, args.name, undefined);
  }

  /** Convenience: build a single-file skill from a `SKILL.md` string. */
  static async fromContent(skillMd: string, args: { readonly name?: string } = {}): Promise<Skill> {
    if (typeof skillMd !== "string" || skillMd.length === 0) {
      throw new Error("Skill.fromContent: skillMd must be a non-empty string");
    }
    return Skill.#fromFiles("Skill.fromContent", { "SKILL.md": skillMd }, args.name, undefined);
  }

  /**
   * Build a draft skill from an already-zipped bundle. The zip is unpacked to a
   * files map (directory entries dropped), then re-canonicalised so a
   * `fromBytes` skill and the identical `fromDir` / `fromFiles` skill dedup by
   * content hash. Requires `SKILL.md` at the archive root.
   */
  static async fromBytes(args: { readonly name?: string; readonly zip: Uint8Array }): Promise<Skill> {
    if (!args || !(args.zip instanceof Uint8Array) || args.zip.byteLength === 0) {
      throw new Error("Skill.fromBytes: { zip } must be a non-empty Uint8Array");
    }
    let entries: Record<string, Uint8Array>;
    try {
      entries = unzipSync(args.zip);
    } catch (err) {
      throw new Error(`Skill.fromBytes: could not unzip the bundle (expected a .zip): ${(err as Error).message}`);
    }
    const files: Record<string, Uint8Array> = {};
    for (const [rawPath, bytes] of Object.entries(entries)) {
      const path = rawPath.replace(/\\/g, "/");
      if (path.endsWith("/")) continue; // directory entry
      files[path] = bytes;
    }
    return Skill.#fromFiles("Skill.fromBytes", files, args.name, undefined);
  }

  // --- workspace upsert -----------------------------------------------------

  /**
   * UPSERT this skill into the workspace registry by name and return an uploaded
   * `Skill` whose ref is `{ kind:"skill", name }`. Two steps: stage the bytes to
   * the content-addressed asset store (dedup makes identical bytes a no-op PUT),
   * then PUT the registry entry (identical `contentHash` ⇒ server no-op). Reusing
   * the SAME instance across submits skips both round-trips.
   *
   * Throws on an already-uploaded (non-draft) skill, mirroring `Tool.upload`.
   */
  async upload(client: SkillUploader): Promise<Skill> {
    const bundle = this._takeDraftBundle();
    if (!bundle) {
      throw new Error(
        "Skill.upload: only draft skills can be uploaded. This skill is already a workspace ref " +
          "({ kind:'skill', name }); reference it by name in skills:[...]."
      );
    }
    if (this.#uploadedName === undefined) {
      await client._uploadAsset({
        bytes: bundle.bytes,
        hash: bundle.contentHash,
        contentType: "application/zip"
      });
      await client._upsertSkill({
        name: bundle.name,
        contentHash: bundle.contentHash,
        description: bundle.description,
        sizeBytes: bundle.bytes.byteLength
      });
      this.#uploadedName = bundle.name;
    }
    return new Skill({ kind: "skill", name: bundle.name }, bundle.description);
  }

  /**
   * Internal: yield the draft's bytes + metadata so `client.run` / `openSession`
   * can auto-upsert it. Non-consuming: a Skill is reusable across sessions — the
   * first use caches the resolved name so later uses skip the round-trip. Returns
   * undefined for an already-uploaded skill.
   */
  _takeDraftBundle(): { name: string; description: string; contentHash: string; bytes: Uint8Array } | undefined {
    if (this.#ref.kind !== "draft" || !this.#bundleBytes) {
      return undefined;
    }
    return {
      name: this.#ref.name,
      description: this.#ref.description,
      contentHash: this.#ref.contentHash,
      bytes: this.#bundleBytes
    };
  }

  toJSON(): SkillRef {
    if (this.#ref.kind === "draft") {
      throw new Error(
        "Skill: a draft skill cannot be JSON-serialised — it only becomes a by-name wire ref once " +
          "skill.upload(client) upserts it (or run / openSession auto-upserts it from skills:[...])."
      );
    }
    return this.#ref;
  }

  static async #fromFiles(
    source: string,
    files: SkillFiles,
    explicitName: string | undefined,
    dirBasename: string | undefined
  ): Promise<Skill> {
    const front = extractSkillFrontmatter(source, files);
    const name = deriveSkillName(source, front.name, explicitName, dirBasename);
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
    const ref: DraftSkillRef = { kind: "draft", name, description, contentHash };
    return new Skill(ref, description, bundled.zip);
  }
}

/**
 * SDK-internal draft marker. Never reaches the wire; `skill.upload` /
 * `run` / `openSession` converts it to a by-name {@link SkillRef} once the bytes
 * are upserted.
 */
export interface DraftSkillRef {
  readonly kind: "draft";
  readonly name: string;
  readonly description: string;
  readonly contentHash: string;
}

/**
 * Minimal client surface `skill.upload` needs to upsert a workspace skill.
 * `Aex` satisfies it; defined structurally here so `skill.ts` does not import
 * `client.ts` (which would be circular — `client.ts` imports `Skill`).
 */
export interface SkillUploader {
  _uploadAsset(args: {
    readonly bytes: Uint8Array;
    readonly hash: string;
    readonly contentType?: string;
  }): Promise<{ readonly assetId: string }>;
  _upsertSkill(args: {
    readonly name: string;
    readonly contentHash: string;
    readonly description: string;
    readonly sizeBytes: number;
  }): Promise<{ readonly updated: boolean }>;
}

/**
 * Resolve a skill name: `{ name }` arg → SKILL.md frontmatter `name:` →
 * (fromDir only) slugified directory basename; else error. Then validate the
 * pattern, reject the `__` MCP separator, and reject reserved names. Never
 * returns an invalid name silently — an underivable / non-conforming name throws.
 */
function deriveSkillName(
  source: string,
  frontmatterName: string | undefined,
  explicitName: string | undefined,
  dirBasename: string | undefined
): string {
  let name: string | undefined = explicitName ?? frontmatterName;
  if (name === undefined && dirBasename !== undefined) {
    const slug = slugifyName(dirBasename);
    if (slug.length > 0) {
      name = slug;
    }
  }
  if (typeof name !== "string" || name.length === 0) {
    throw new Error(
      `${source}: a skill name is required — pass { name }, add a \`name:\` field to the SKILL.md ` +
        `YAML frontmatter, or (for fromDir) use a directory whose basename slugifies to a valid name`
    );
  }
  if (!SKILL_NAME_PATTERN.test(name)) {
    throw new Error(`${source}: name ${JSON.stringify(name)} must match ${SKILL_NAME_PATTERN.source}`);
  }
  if (name.includes("__")) {
    throw new Error(`${source}: name must not contain "__"; that separator is reserved for MCP tools`);
  }
  if (SKILL_RESERVED_NAMES.has(name)) {
    throw new Error(
      `${source}: name ${JSON.stringify(name)} is reserved (${[...SKILL_RESERVED_NAMES].join(", ")}); pick another`
    );
  }
  return name;
}

/** Lowercase, collapse non-`[a-z0-9]` runs to `-`, trim leading/trailing `-`. */
function slugifyName(input: string): string {
  return input.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
}

/** Directory basename of a filesystem path (handles `/` and `\`, trailing slashes). */
function dirBasename(path: string): string {
  const normalised = path.replace(/\\/g, "/").replace(/\/+$/, "");
  return normalised.split("/").at(-1) ?? "";
}

/**
 * Read `SKILL.md` from a bundle files map and parse its YAML frontmatter for
 * `name` + `description`. Throws when the bundle has no root `SKILL.md` (that is
 * what makes a bundle a skill).
 */
function extractSkillFrontmatter(source: string, files: SkillFiles): { name?: string; description?: string } {
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
 * stripped); anything else is ignored.
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
