/**
 * Schemas for `submission.assets` — the four lists of immutable, version-pinned
 * workspace resources a run materializes.
 *
 * Each list is an array of a per-kind element schema, so the wire shape of a
 * submitted resource is stated once here rather than reconstructed field by
 * field in `submission.ts`.
 *
 * **What deliberately stays in the parser** (D4/L1): the *name grammars*
 * (`assertWorkspaceFileResourceName`, the skill and tool name rules), the mount
 * path assert, the `assetId`/`contentHash` agreement, the `entry` bundle-path
 * normalisation and per-list duplicate detection. The grammars are asserts the
 * SDK builders call directly on values they already hold, and the skill/tool
 * ones are declared in `session-config.ts`, which imports `submission.ts` — a
 * cycle if this module reached for them. Everything a schema *can* state — the
 * kind discriminator, the pinned identity quartet, string/number envelopes,
 * description bounds, the tool input schema — is stated here.
 *
 * Messages carry the element's INDEX (`submission.assets.files[3].mountPath`),
 * which is read off the position Zod reports rather than baked in, so one
 * element schema serves the whole list at any mount depth — see `indexedPath` /
 * `indexedFieldPath` in `wire.ts`.
 */
import * as z from "zod/mini";
import { isJsonValue } from "../value-guards.js";
import { indexedFieldPath, indexedPath, wireObject, type WireObjectText } from "./wire.js";

const ASSETS = "submission.assets";

/** Maximum length of a resource description shown to the model. */
const RESOURCE_DESCRIPTION_MAX_CHARS = 2048;

/**
 * A rejected key on a resource element names the key and stops — the permitted
 * list is deliberately absent, because the per-kind list differs across the four
 * lists and the caller already knows which list it wrote to.
 */
const resourceRefText: Partial<WireObjectText> = {
  unknownKey: (path, key) => `${path}.${key} is not allowed`
};

/** A required, non-empty string field of the element at an indexed position. */
function refString(list: string) {
  const message = (issue: { readonly path?: readonly PropertyKey[] | undefined }): string =>
    `${indexedFieldPath(list, issue.path)} must be a non-empty string`;
  return z.string({ error: message }).check(z.minLength(1, { error: message, abort: true }));
}

/** The pinned `version` counter: a positive safe integer. */
function refVersion(list: string) {
  const message = (issue: { readonly path?: readonly PropertyKey[] | undefined }): string =>
    `${indexedFieldPath(list, issue.path)} must be a positive integer`;
  return z
    .number({ error: message })
    .check(
      z.refine((value: number) => Number.isSafeInteger(value) && value >= 1, {
        error: message,
        abort: true
      })
    );
}

/**
 * The kind discriminator. Declared first so a mislabelled element is rejected
 * before its identity fields are read, matching the parser's order.
 */
function refKind<Kind extends string>(list: string, kind: Kind) {
  return z.literal(kind, {
    error: (issue) => `${indexedFieldPath(list, issue.path)} must be '${kind}'`
  });
}

/**
 * A model-facing description: non-empty after trimming and within
 * {@link RESOURCE_DESCRIPTION_MAX_CHARS}. The envelope and the bound report
 * separately, as the `requireResourceDescription` ladder they replace did.
 */
function refDescription(list: string) {
  const envelope = (issue: { readonly path?: readonly PropertyKey[] | undefined }): string =>
    `${indexedFieldPath(list, issue.path)} must be a non-empty string`;
  const bound = (issue: { readonly path?: readonly PropertyKey[] | undefined }): string =>
    `${indexedFieldPath(list, issue.path)} must be non-empty and <= ${RESOURCE_DESCRIPTION_MAX_CHARS} chars`;
  return z.string({ error: envelope }).check(
    z.minLength(1, { error: envelope, abort: true }),
    z.refine(
      (value: string) =>
        value.trim().length > 0 && value.length <= RESOURCE_DESCRIPTION_MAX_CHARS,
      { error: bound, abort: true }
    )
  );
}

/** A custom tool's JSON Schema: an object literal whose `type` is `"object"`. */
function refInputSchema(list: string) {
  const notObject = (issue: { readonly path?: readonly PropertyKey[] | undefined }): string =>
    `${indexedFieldPath(list, issue.path)} must be an object`;
  const notToolSchema = (issue: { readonly path?: readonly PropertyKey[] | undefined }): string =>
    `${indexedFieldPath(list, issue.path)} must be a JSON Schema object with type 'object'`;
  return z.record(z.string(), z.unknown(), { error: notObject }).check(
    z.refine(
      (value: Record<string, unknown>) => isJsonValue(value) && value.type === "object",
      { error: notToolSchema, abort: true }
    )
  );
}

/** The immutable identity every submitted resource carries, in assertion order. */
function pinnedRefKeys<Kind extends string>(list: string, kind: Kind) {
  return {
    kind: refKind(list, kind),
    resourceId: refString(list),
    version: refVersion(list),
    assetId: refString(list),
    contentHash: refString(list)
  };
}

/**
 * One resource element, strict over its per-kind key set.
 *
 * Declaration order is the order the sequential assertions this replaces ran in,
 * and `errorFromZod` reports the shallowest issue first, so a rejected key still
 * outranks a field complaint on the same element.
 */
function refObject<Shape extends z.core.$ZodLooseShape>(list: string, shape: Shape) {
  return wireObject(indexedPath(list), shape, resourceRefText);
}

const FILES = `${ASSETS}.files`;
const SKILLS = `${ASSETS}.skills`;
const TOOLS = `${ASSETS}.tools`;
const INSTRUCTIONS = `${ASSETS}.instructions`;

/** `submission.assets.files[i]` — a mounted workspace file. */
export const WorkspaceFileRefSchema = refObject(FILES, {
  ...pinnedRefKeys(FILES, "file"),
  name: refString(FILES),
  mountPath: refString(FILES)
});

/** `submission.assets.skills[i]` — a loadable skill bundle. */
export const WorkspaceSkillRefSchema = refObject(SKILLS, {
  ...pinnedRefKeys(SKILLS, "skill"),
  name: refString(SKILLS),
  description: refDescription(SKILLS)
});

/** `submission.assets.tools[i]` — a custom tool bundle plus its input schema. */
export const WorkspaceToolRefSchema = refObject(TOOLS, {
  ...pinnedRefKeys(TOOLS, "tool"),
  name: refString(TOOLS),
  description: refDescription(TOOLS),
  input_schema: refInputSchema(TOOLS),
  entry: refString(TOOLS)
});

/**
 * `submission.assets.instructions[i]` — an injected instruction document.
 *
 * Pinned to TEXT, not to bytes, so it deliberately does not share
 * {@link pinnedRefKeys}: no `assetId`, no `contentHash`, and `textHash` in
 * their place. The key set is strict, so a caller still sending the retired
 * asset pair is rejected by name rather than having it silently ignored.
 */
export const WorkspaceInstructionRefSchema = refObject(INSTRUCTIONS, {
  kind: refKind(INSTRUCTIONS, "instruction"),
  resourceId: refString(INSTRUCTIONS),
  version: refVersion(INSTRUCTIONS),
  textHash: refString(INSTRUCTIONS),
  name: refString(INSTRUCTIONS)
});

/**
 * The four asset lists.
 *
 * Its unknown-key sentence is deliberately the shorter "is not allowed" wording
 * rather than the "is not an allowed field" the siblings use; both are pinned
 * byte-for-byte by `test/allowed-keys-parser-golden.test.ts`, so neither can be
 * normalised toward the other without a contract change.
 */
export const SubmissionAssetsSchema = wireObject(
  ASSETS,
  {
    files: z.optional(z.array(WorkspaceFileRefSchema, { error: `${FILES} must be an array` })),
    skills: z.optional(z.array(WorkspaceSkillRefSchema, { error: `${SKILLS} must be an array` })),
    tools: z.optional(z.array(WorkspaceToolRefSchema, { error: `${TOOLS} must be an array` })),
    instructions: z.optional(
      z.array(WorkspaceInstructionRefSchema, { error: `${INSTRUCTIONS} must be an array` })
    )
  },
  {
    unknownKey: (path, key, permitted) =>
      `${path}.${key} is not allowed; permitted: ${permitted.join(", ")}`
  }
).register(z.globalRegistry, {
  id: "SubmissionAssets",
  description:
    "Immutable, version-pinned workspace resources materialized for the run. Resource name " +
    "grammars, the mount-path rules, the assetId/contentHash agreement and per-list duplicate " +
    "detection are enforced in packages/contracts/src/submission.ts and are not expressible here."
});

export type SubmissionAssetsWire = z.infer<typeof SubmissionAssetsSchema>;
export type WorkspaceFileRefWire = z.infer<typeof WorkspaceFileRefSchema>;
export type WorkspaceSkillRefWire = z.infer<typeof WorkspaceSkillRefSchema>;
export type WorkspaceToolRefWire = z.infer<typeof WorkspaceToolRefSchema>;
export type WorkspaceInstructionRefWire = z.infer<typeof WorkspaceInstructionRefSchema>;
