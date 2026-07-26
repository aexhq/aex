import {
  MOUNT_PATH_MAX_LENGTH,
  MOUNT_PATH_PATTERN,
  SKILL_BUNDLE_LIMITS,
  SKILL_NAME_PATTERN,
  SKILL_RESERVED_NAMES,
  TOOL_NAME_PATTERN,
  assertValidMountPath,
  normaliseSkillBundlePath
} from "./session-config.js";
import {
  BUILTIN_TOOL_NAMES,
  type BuiltinToolName,
  type BuiltinToolsSelection
} from "./submission.js";
import { isJsonValue, isRecord, type JsonValue } from "./value-guards.js";
import {
  WORKSPACE_FILE_RESOURCE_NAME_PATTERN,
  WORKSPACE_INSTRUCTION_RESOURCE_NAME_PATTERN,
  assertPinnedWorkspaceResource,
  assertWorkspaceFileResourceName,
  assertWorkspaceInstructionResourceName,
  type SubmissionAssets,
  type WorkspaceFileRef,
  type WorkspaceInstructionRef,
  type WorkspaceResourceRef,
  type WorkspaceSkillRef,
  type WorkspaceToolRef
} from "./workspace-resources.js";

/** JSON-schema fragment used by the private hosted subagent tool definition. */
export type SubagentInputSchema = Readonly<Record<string, JsonValue>>;

const ASSET_COLLECTIONS = ["files", "skills", "tools", "instructions"] as const satisfies readonly (keyof SubmissionAssets)[];
type AssetCollection = (typeof ASSET_COLLECTIONS)[number];

type ResourceDescriptorMap = {
  readonly [Collection in AssetCollection]: {
    readonly kind: SubmissionAssets[Collection][number]["kind"];
    /**
     * The JSON-schema property map, required to name EVERY key of the ref type
     * and no others.
     *
     * This mapped type is what the retired `allowed-keys` helper used to
     * provide — a compile-time proof that a runtime key list matches the
     * interface — except it proves it about the list that already had to exist.
     * The allow-list and the required-key list are now read off this one
     * declaration rather than restated beside it, so the three cannot disagree.
     */
    readonly properties: { readonly [Key in keyof SubmissionAssets[Collection][number]]-?: JsonValue };
  };
};

/** The permitted keys of a resource ref, in declaration order. */
function descriptorKeys(collection: AssetCollection): readonly string[] {
  return Object.keys(RESOURCE_DESCRIPTORS[collection].properties);
}

/**
 * Reject the first key the caller sent that the allow-list does not name.
 *
 * Local rather than shared with the wire schemas: this validates a MODEL tool
 * argument against a JSON Schema this module also advertises, not an HTTP wire
 * object, and its diagnostics are prose for a model rather than the wire's
 * `permitted: …` form. What matters is that the key list it checks against is
 * derived from the advertised `properties` — so the schema the model is shown
 * and the rule it is held to are one declaration.
 */
function rejectUnknownKeys(
  record: Readonly<Record<string, unknown>>,
  permitted: readonly string[],
  message: (key: string) => string
): void {
  for (const key of Object.keys(record)) {
    if (!permitted.includes(key)) {
      throw new Error(message(key));
    }
  }
}

const COMMON_PROPERTIES = {
  resourceId: { type: "string", pattern: "^wres_[0-9a-f]{32}$" },
  version: { type: "integer", minimum: 1, maximum: Number.MAX_SAFE_INTEGER },
  assetId: { type: "string", pattern: "^asset_[0-9a-f]{64}$" },
  contentHash: { type: "string", pattern: "^sha256:[0-9a-f]{64}$" }
} as const satisfies Readonly<Record<string, JsonValue>>;

const RESOURCE_DESCRIPTORS = {
  files: {
    kind: "file",
    properties: {
      kind: { const: "file" },
      ...COMMON_PROPERTIES,
      name: {
        type: "string",
        pattern: WORKSPACE_FILE_RESOURCE_NAME_PATTERN.source,
        maxLength: 128
      },
      mountPath: {
        type: "string",
        pattern: MOUNT_PATH_PATTERN.source,
        minLength: 1,
        maxLength: MOUNT_PATH_MAX_LENGTH
      }
    }
  },
  skills: {
    kind: "skill",
    properties: {
      kind: { const: "skill" },
      ...COMMON_PROPERTIES,
      name: { type: "string", pattern: SKILL_NAME_PATTERN.source, maxLength: 128 },
      description: { type: "string", minLength: 1, maxLength: 2048, pattern: "\\S" }
    }
  },
  tools: {
    kind: "tool",
    properties: {
      kind: { const: "tool" },
      ...COMMON_PROPERTIES,
      name: { type: "string", pattern: TOOL_NAME_PATTERN.source, maxLength: 128 },
      description: { type: "string", minLength: 1, maxLength: 2048, pattern: "\\S" },
      input_schema: {
        type: "object",
        properties: { type: { const: "object" } },
        required: ["type"]
      },
      entry: {
        type: "string",
        minLength: 1,
        maxLength: SKILL_BUNDLE_LIMITS.maxPathLength
      }
    }
  },
  instructions: {
    kind: "instruction",
    properties: {
      kind: { const: "instruction" },
      ...COMMON_PROPERTIES,
      name: {
        type: "string",
        pattern: WORKSPACE_INSTRUCTION_RESOURCE_NAME_PATTERN.source,
        maxLength: 128
      }
    }
  }
} as const satisfies ResourceDescriptorMap;

/** Build the model-advertised closed assets object from the runtime descriptor. */
export function buildSubagentAssetsInputSchema(): SubagentInputSchema {
  const properties: Record<string, JsonValue> = {};
  for (const collection of ASSET_COLLECTIONS) {
    const descriptor = RESOURCE_DESCRIPTORS[collection];
    properties[collection] = {
      type: "array",
      items: {
        type: "object",
        properties: descriptor.properties,
        required: [...descriptorKeys(collection)],
        additionalProperties: false
      }
    };
  }
  return {
    type: "object",
    properties,
    required: [...ASSET_COLLECTIONS],
    additionalProperties: false
  };
}

/** Build the model-advertised builtin selection from the runtime name tuple. */
export function buildSubagentBuiltinToolsInputSchema(): SubagentInputSchema {
  return {
    oneOf: [
      { type: "string", enum: ["default", "none"] },
      { type: "array", items: { type: "string", enum: [...BUILTIN_TOOL_NAMES] } }
    ],
    description: "Builtin capabilities for the child: default, none, or an exact list of builtin tool names."
  };
}

/**
 * Parse the complete optional subagent assets argument without normalizing valid
 * caller bytes. The API remains authoritative for workspace-backed facts.
 */
export function parseSubagentAssetsInput(input: unknown): SubmissionAssets | undefined {
  if (input === undefined) return undefined;
  if (!isRecord(input)) {
    throw new Error("subagent: `assets` must contain files, skills, tools, and instructions arrays");
  }
  rejectUnknownKeys(input, ASSET_COLLECTIONS, (key) => `subagent: assets.${key} is not allowed`);

  const parsed = {
    files: parseResourceCollection(input.files, "files"),
    skills: parseResourceCollection(input.skills, "skills"),
    tools: parseResourceCollection(input.tools, "tools"),
    instructions: parseResourceCollection(input.instructions, "instructions")
  } satisfies SubmissionAssets;

  const seen = new Set<string>();
  for (const collection of ASSET_COLLECTIONS) {
    for (let index = 0; index < parsed[collection].length; index += 1) {
      const ref = parsed[collection][index] as WorkspaceResourceRef;
      const identity = `${ref.resourceId}:${ref.version}`;
      if (seen.has(identity)) {
        throw new Error(`subagent: assets.${collection}[${index}] duplicates resource version ${identity}`);
      }
      seen.add(identity);
    }
  }
  return parsed;
}

/** Parse the optional subagent builtin selection while retaining local defaults. */
export function parseSubagentBuiltinToolsInput(input: unknown): BuiltinToolsSelection | undefined {
  if (input === undefined) return undefined;
  if (input === "default" || input === "none") return input;
  if (
    Array.isArray(input) &&
    hasEveryArrayIndex(input) &&
    input.every(
      (entry): entry is BuiltinToolName =>
        typeof entry === "string" && (BUILTIN_TOOL_NAMES as readonly string[]).includes(entry)
    )
  ) {
    return [...input];
  }
  throw new Error("subagent: `builtinTools` must be 'default', 'none', or an array of builtin names");
}

function parseResourceCollection<Collection extends AssetCollection>(
  input: unknown,
  collection: Collection
): SubmissionAssets[Collection] {
  if (!Array.isArray(input)) {
    throw new Error(`subagent: assets.${collection} must be an array`);
  }
  return Array.from(input, (entry, index) => parseResource(entry, collection, index)) as unknown as SubmissionAssets[Collection];
}

function parseResource(
  input: unknown,
  collection: AssetCollection,
  index: number
): WorkspaceResourceRef {
  const descriptor = RESOURCE_DESCRIPTORS[collection];
  const path = `subagent: assets.${collection}[${index}]`;
  if (!isRecord(input)) throw new Error(`${path} must be an object`);
  rejectUnknownKeys(input, descriptorKeys(collection), (key) => `${path}.${key} is not allowed`);
  if (input.kind !== descriptor.kind) throw new Error(`${path}.kind must be '${descriptor.kind}'`);

  const base = {
    resourceId: requireString(input.resourceId, `${path}.resourceId`),
    version: requirePositiveInteger(input.version, `${path}.version`),
    assetId: requireString(input.assetId, `${path}.assetId`),
    contentHash: requireString(input.contentHash, `${path}.contentHash`)
  };

  let result: WorkspaceResourceRef;
  switch (collection) {
    case "files": {
      const name = requireString(input.name, `${path}.name`);
      assertWorkspaceFileResourceName(name, `${path}.name`);
      const mountPath = requireString(input.mountPath, `${path}.mountPath`);
      assertValidMountPath(mountPath, `${path}.mountPath`);
      result = { ...base, kind: "file", name, mountPath };
      break;
    }
    case "skills": {
      const name = requireString(input.name, `${path}.name`);
      assertSkillName(name, `${path}.name`);
      const description = requireDescription(input.description, `${path}.description`);
      result = { ...base, kind: "skill", name, description };
      break;
    }
    case "tools": {
      const name = requireString(input.name, `${path}.name`);
      if (!TOOL_NAME_PATTERN.test(name) || name.includes("__")) {
        throw new Error(`${path}.name must be a non-reserved tool name matching ${TOOL_NAME_PATTERN.source}`);
      }
      const description = requireDescription(input.description, `${path}.description`);
      if (!isRecord(input.input_schema) || !isJsonValue(input.input_schema) || input.input_schema.type !== "object") {
        throw new Error(`${path}.input_schema must be a JSON Schema object with type 'object'`);
      }
      const entry = requireString(input.entry, `${path}.entry`);
      assertToolEntry(entry, `${path}.entry`);
      result = { ...base, kind: "tool", name, description, input_schema: input.input_schema, entry };
      break;
    }
    case "instructions": {
      const name = requireString(input.name, `${path}.name`);
      assertWorkspaceInstructionResourceName(name, `${path}.name`);
      result = { ...base, kind: "instruction", name };
      break;
    }
  }
  assertPinnedWorkspaceResource(result, path);
  return input as unknown as WorkspaceResourceRef;
}

function requireString(input: unknown, path: string): string {
  if (typeof input !== "string" || input.length === 0) {
    throw new Error(`${path} must be a non-empty string`);
  }
  return input;
}

function requirePositiveInteger(input: unknown, path: string): number {
  if (!Number.isSafeInteger(input) || (input as number) < 1) {
    throw new Error(`${path} must be a positive integer`);
  }
  return input as number;
}

function requireDescription(input: unknown, path: string): string {
  const value = requireString(input, path);
  if (value.trim().length === 0 || value.length > 2048) {
    throw new Error(`${path} must be non-empty and <= 2048 chars`);
  }
  return value;
}

function assertSkillName(name: string, path: string): void {
  if (!SKILL_NAME_PATTERN.test(name)) {
    throw new Error(`${path} must match ${SKILL_NAME_PATTERN.source}`);
  }
  if (name.includes("__")) {
    throw new Error(`${path} must not contain "__"; that separator is reserved for MCP tools`);
  }
  if (SKILL_RESERVED_NAMES.has(name)) {
    throw new Error(`${path} must not be a reserved skills name (${[...SKILL_RESERVED_NAMES].sort().join(", ")})`);
  }
}

function hasEveryArrayIndex(input: readonly unknown[]): boolean {
  for (let index = 0; index < input.length; index += 1) {
    if (!Object.prototype.hasOwnProperty.call(input, index)) return false;
  }
  return true;
}

function assertToolEntry(entry: string, path: string): void {
  try {
    normaliseSkillBundlePath(entry);
  } catch (error) {
    const message = error instanceof Error ? error.message : "bundle entry path is invalid";
    const detail = message
      .replace(/^bundle entry path /u, "")
      .replace(/:.*$/su, "");
    throw new Error(`${path} ${detail}`);
  }
}
