import {
  SKILL_NAME_PATTERN,
  SKILL_RESERVED_NAMES,
  TOOL_NAME_PATTERN,
  normaliseSkillBundlePath,
  type ToolInputSchema
} from "./session-config.js";
import type { SkillFiles, ToolBundleManifest } from "./asset-bundle.js";

export interface AssetToolManifestInput {
  readonly name: string;
  readonly description: string;
  readonly inputSchema?: ToolInputSchema;
  readonly input_schema?: ToolInputSchema;
  readonly entry: string;
}

export interface SkillFrontmatter {
  readonly name?: string;
  readonly description?: string;
}

/** Internal authoring helper shared by the SDK and the bundled CLI. */
export function extractSkillFrontmatter(source: string, files: SkillFiles): SkillFrontmatter {
  const raw = files["SKILL.md"];
  if (raw === undefined) {
    throw new Error(`${source}: the skill bundle must contain a SKILL.md at its root`);
  }
  const text = typeof raw === "string" ? raw : new TextDecoder().decode(raw);
  return parseSkillFrontmatter(text);
}

/** Minimal scalar-only YAML frontmatter reader used by public skill factories. */
export function parseSkillFrontmatter(text: string): SkillFrontmatter {
  const src = text.charCodeAt(0) === 0xfeff ? text.slice(1) : text;
  const match = /^---[ \t]*\r?\n([\s\S]*?)\r?\n---[ \t]*(?:\r?\n|$)/.exec(src);
  if (!match) return {};
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
    if (value.length > 0) out[key] = value;
  }
  return out;
}

/** Resolve and validate the canonical skill name shared by SDK and CLI drafts. */
export function deriveSkillName(
  source: string,
  frontmatterName: string | undefined,
  explicitName: string | undefined,
  dirBasename: string | undefined,
  diagnostics: "sdk" | "cli" = "sdk"
): string {
  let name: string | undefined = explicitName ?? frontmatterName;
  if (name === undefined && dirBasename !== undefined) {
    const slug = slugifyAssetName(dirBasename);
    if (slug.length > 0) name = slug;
  }
  if (typeof name !== "string" || name.length === 0) {
    if (diagnostics === "cli") {
      throw new Error(`${source}: a skill name is required`);
    }
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
    if (diagnostics === "cli") {
      throw new Error(`${source}: name ${JSON.stringify(name)} is reserved (${[...SKILL_RESERVED_NAMES].join(", ")})`);
    }
    throw new Error(
      `${source}: name ${JSON.stringify(name)} is reserved (${[...SKILL_RESERVED_NAMES].join(", ")}); pick another`
    );
  }
  return name;
}

/** Validate and canonicalize a tool manifest before it is archived. */
export function normalizeToolManifest(
  source: string,
  input: AssetToolManifestInput,
  files?: SkillFiles,
  diagnostics: "sdk" | "cli" = "sdk"
): ToolBundleManifest {
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
  assertJsModuleEntry(source, entry, input.entry, files, diagnostics);
  return { name, description, input_schema: inputSchema, entry };
}

export function slugifyAssetName(input: string): string {
  return input.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
}

const JS_MODULE_ENTRY = /\.(?:js|mjs|cjs)$/i;

function assertJsModuleEntry(
  source: string,
  entry: string,
  rawEntry: string,
  files: SkillFiles | undefined,
  diagnostics: "sdk" | "cli"
): void {
  const basename = entry.split("/").pop() ?? entry;
  if (!JS_MODULE_ENTRY.test(basename)) {
    if (diagnostics === "cli") {
      throw new Error(`${source}: entry must be a JS module (.js/.mjs/.cjs)`);
    }
    throw new Error(
      `${source}: entry must be a JS module (.js/.mjs/.cjs) that default-exports a function or { execute }; got ${JSON.stringify(rawEntry)}`
    );
  }
  if (files !== undefined && !(entry in files) && !(rawEntry in files)) {
    if (diagnostics === "cli") {
      throw new Error(`${source}: entry ${JSON.stringify(rawEntry)} is not present in files`);
    }
    throw new Error(
      `${source}: entry ${JSON.stringify(rawEntry)} is not present in files (keys: ${Object.keys(files).join(", ") || "(none)"})`
    );
  }
}
