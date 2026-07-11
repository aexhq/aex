import {
  SKILL_NAME_PATTERN,
  SKILL_RESERVED_NAMES,
  type FetchLike
} from "@aexhq/contracts";
import { bundleSkillFiles, hashSkillBundle, type BundleMeta, type SkillFiles } from "./bundle.js";
import { fetchSkillArchive } from "./fetch-archive.js";
import { readDirectoryWithFidelity } from "./node-fs.js";
import type { IgnoreOptions } from "./node-walk.js";
import { unzipSync } from "fflate";

/**
 * A Skill is a draft workspace bundle of instructional /
 * executable content (`SKILL.md` at the bundle root plus any supporting files).
 * It is distinct from a {@link Tool}: a session gets a single `skills` meta-tool
 * (list/load) rather than one load-tool per skill.
 *
 * Factories read, canonically bundle, and hash local content. Publish the draft
 * with `aex.workspace.skills.publish(skill)`, then pin the returned immutable
 * resource ref in `assets.skills`.
 */
export class Skill {
  readonly #ref: DraftSkillRef;
  readonly #description: string;
  readonly #bundleBytes: Uint8Array;

  /** Internal constructor. Use the `Skill.from*` factories. */
  private constructor(ref: DraftSkillRef, description: string, bundleBytes: Uint8Array) {
    this.#ref = ref;
    this.#description = description;
    this.#bundleBytes = bundleBytes;
  }

  /** Draft identity and canonical content hash. */
  get ref(): DraftSkillRef {
    return this.#ref;
  }

  /** True for a local-bytes skill that has not been upserted to the workspace yet. */
  get isDraft(): boolean {
    return true;
  }

  get name(): string {
    return this.#ref.name;
  }

  get description(): string {
    return this.#description;
  }

  // --- factories (source symmetry with File / Instructions / Tool) --------------

  /**
   * Read a local skill directory. It must contain `SKILL.md` at its root, whose
   * YAML frontmatter supplies `description` and (unless `args.name` is given)
   * `name`. When neither an explicit name nor a frontmatter name is present, the
   * slugified directory basename is used.
   *
   * The directory is walked with FIDELITY (the same walk `File.fromPath` uses):
   * `.aexignore` + the always-on defaults (`.git/`, `node_modules/`, …) prune the
   * upload — so a skill dir carrying `node_modules` no longer ships it — and
   * executable bits + symlinks are captured into a `.aexmeta.json` sidecar
   * (emitted only when such metadata exists, so a pure-content skill dir stays
   * byte-identical → dedup continuity). Bun/Node filesystem runtimes only.
   */
  static async fromDir(
    rootDir: string,
    args: { readonly name?: string; readonly ignore?: IgnoreOptions } = {}
  ): Promise<Skill> {
    const { files, meta } = await readDirectoryWithFidelity(rootDir, args.ignore);
    return Skill.#fromFiles("Skill.fromDir", files, args.name, dirBasename(rootDir), meta);
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
   * Requires a root `SKILL.md`. Universal (no filesystem access). An optional
   * `meta` (exec bits + symlinks) is threaded to the bundler for callers that
   * carry fidelity metadata alongside a hand-built map; omit it and a plain map
   * stays byte-identical to the pre-fidelity output.
   */
  static async fromFiles(args: {
    readonly name?: string;
    readonly files: SkillFiles;
    readonly meta?: BundleMeta;
  }): Promise<Skill> {
    if (!args || typeof args !== "object" || args.files === undefined) {
      throw new Error("Skill.fromFiles: { files } is required");
    }
    return Skill.#fromFiles("Skill.fromFiles", args.files, args.name, undefined, args.meta);
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

  /**
   * Internal: yield the draft bytes and metadata to the workspace publisher.
   */
  _takeDraftBundle(): { name: string; description: string; contentHash: string; bytes: Uint8Array } {
    return {
      name: this.#ref.name,
      description: this.#ref.description,
      contentHash: this.#ref.contentHash,
      bytes: this.#bundleBytes
    };
  }

  toJSON(): never {
    throw new Error("Skill drafts cannot be submitted directly; publish with aex.workspace.skills.publish(...)");
  }

  static async #fromFiles(
    source: string,
    files: SkillFiles,
    explicitName: string | undefined,
    dirBasename: string | undefined,
    meta?: BundleMeta
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
    const bundled = bundleSkillFiles(files, meta);
    const contentHash = await hashSkillBundle(bundled.zip);
    const ref: DraftSkillRef = { kind: "draft", name, description, contentHash };
    return new Skill(ref, description, bundled.zip);
  }
}

/**
 * SDK-internal draft marker. It never reaches the session wire directly.
 */
export interface DraftSkillRef {
  readonly kind: "draft";
  readonly name: string;
  readonly description: string;
  readonly contentHash: string;
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
