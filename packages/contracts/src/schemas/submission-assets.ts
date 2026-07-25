/**
 * Schemas for `submission.assets` — the four lists of immutable, version-pinned
 * workspace resources a run materializes, and the per-element key gate for each.
 *
 * **Key gate only.** Every member is {@link unspecifiedField}: the KEYS are
 * declared once here, while the value rules (name grammars, mount paths, the
 * pinned identity quartet, per-list duplicate detection) still live in
 * `submission.ts` and in `./workspace-resources.ts`. See D4/L1 — this is a
 * migration state, and each list's element schema replaces its
 * `unspecifiedField`s as that family's field validation is ported.
 *
 * Each element gate builds its schema per element rather than reusing one, for
 * two reasons that pull the same way. A resource's wire path carries its
 * position in the list (`submission.assets.files[3].mountPath`), which a schema
 * parsed at the element's own root cannot see. And the parser validates one
 * element at a time — top-down, element by element — so an earlier element's
 * field complaint still outranks a later element's rejected key; a schema built
 * once over the whole ARRAY would report Zod's collected issues instead,
 * inverting that order. One schema per element is the price of keeping both.
 */
import * as z from "zod/mini";
import { parseWire, unspecifiedField, wireObject, type WireObjectText } from "./wire.js";

const ASSETS = "submission.assets";

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
    files: unspecifiedField,
    skills: unspecifiedField,
    tools: unspecifiedField,
    instructions: unspecifiedField
  },
  {
    unknownKey: (path, key, permitted) =>
      `${path}.${key} is not allowed; permitted: ${permitted.join(", ")}`
  }
);

export type SubmissionAssetsWire = z.infer<typeof SubmissionAssetsSchema>;

/**
 * A rejected key on a resource element names the key and stops — the permitted
 * list is deliberately absent, because the per-kind list differs across the four
 * lists and the caller already knows which list it wrote to.
 */
const resourceRefText: Partial<WireObjectText> = {
  unknownKey: (path, key) => `${path}.${key} is not allowed`
};

/** The immutable identity every submitted resource carries, in assertion order. */
const pinnedRefKeys = {
  kind: unspecifiedField,
  resourceId: unspecifiedField,
  version: unspecifiedField,
  assetId: unspecifiedField,
  contentHash: unspecifiedField
};

const workspaceFileRefKeys = {
  ...pinnedRefKeys,
  name: unspecifiedField,
  mountPath: unspecifiedField
};

const workspaceSkillRefKeys = {
  ...pinnedRefKeys,
  name: unspecifiedField,
  description: unspecifiedField
};

const workspaceToolRefKeys = {
  ...pinnedRefKeys,
  name: unspecifiedField,
  description: unspecifiedField,
  input_schema: unspecifiedField,
  entry: unspecifiedField
};

const workspaceInstructionRefKeys = {
  ...pinnedRefKeys,
  name: unspecifiedField
};

/**
 * Gate one resource element: it must be an object, and it may carry only the
 * keys its kind declares. The projection that follows in `submission.ts` reads
 * the returned record field by field, so the values pass through untouched.
 */
export type WorkspaceResourceRefGate = (path: string, input: unknown) => Record<string, unknown>;

function refGate(keys: z.core.$ZodLooseShape): WorkspaceResourceRefGate {
  return (path, input) =>
    parseWire(wireObject(path, keys, resourceRefText), input) as Record<string, unknown>;
}

/** `submission.assets.files[i]` — a mounted workspace file. */
export const gateWorkspaceFileRef: WorkspaceResourceRefGate = refGate(workspaceFileRefKeys);

/** `submission.assets.skills[i]` — a loadable skill bundle. */
export const gateWorkspaceSkillRef: WorkspaceResourceRefGate = refGate(workspaceSkillRefKeys);

/** `submission.assets.tools[i]` — a custom tool bundle plus its input schema. */
export const gateWorkspaceToolRef: WorkspaceResourceRefGate = refGate(workspaceToolRefKeys);

/** `submission.assets.instructions[i]` — an injected instruction document. */
export const gateWorkspaceInstructionRef: WorkspaceResourceRefGate = refGate(
  workspaceInstructionRefKeys
);
