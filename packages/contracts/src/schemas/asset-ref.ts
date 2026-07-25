/**
 * Schemas for the storage-neutral uploaded-asset wire reference and the
 * container directory it unzips into.
 *
 * The id pattern, the mount-path pattern and the mount-path assert are declared
 * here, next to the schema that enforces them, and re-exported by
 * `session-config.ts` under their published names — same reason as the sibling
 * vocabularies: the parser module imports this one, so the constants cannot live
 * there without a cycle.
 */
import * as z from "zod/mini";
import type { AssetRef } from "../session-config.js";
import { parseWire, wireObject } from "./wire.js";

/**
 * Asset ids are storage-neutral product ids. Current uploads derive the id from
 * the content digest (`asset_<sha256hex>`), but callers must treat it as opaque.
 */
export const ASSET_ID_PATTERN = /^asset_[A-Za-z0-9_-]{8,128}$/;

/**
 * A `mountPath` is an ABSOLUTE container directory under the workspace. It must
 * start with `/`, contain no `..`/`.` traversal or NUL/backslash, and stay
 * within {@link MOUNT_PATH_MAX_LENGTH}. The managed runtime rebases it under the
 * workspace root, so a path outside `/workspace` is clamped there — the pattern
 * just rejects obviously-malformed input at the SDK/BFF boundary. A trailing
 * slash is allowed (it is a directory) but not required.
 */
export const MOUNT_PATH_PATTERN = /^\/(?:[^/\0\\]+\/?)*$/;
export const MOUNT_PATH_MAX_LENGTH = 512;

/**
 * Validate a `File.mountPath` (an absolute container directory). Shared by the
 * SDK `File` builders and the BFF asset-ref parser so both reject the same
 * malformed input. Throws with `field` context on failure.
 *
 * Kept as an assert rather than folded into {@link mountPath}: the SDK `File`
 * builders and the workspace-resource parsers call it directly on a value they
 * already hold, with their own field name, and never through a schema.
 */
export function assertValidMountPath(value: string, field: string): void {
  if (value.length === 0 || value.length > MOUNT_PATH_MAX_LENGTH) {
    throw new Error(`${field} must be 1..${MOUNT_PATH_MAX_LENGTH} chars`);
  }
  if (!value.startsWith("/")) {
    throw new Error(`${field} must be an absolute path starting with '/'`);
  }
  if (value.includes("\0") || value.includes("\\")) {
    throw new Error(`${field} must not contain NUL or backslash`);
  }
  if (value.split("/").some((seg) => seg === "..")) {
    throw new Error(`${field} must not contain '..' traversal segments`);
  }
  if (!MOUNT_PATH_PATTERN.test(value)) {
    throw new Error(`${field} must match ${MOUNT_PATH_PATTERN.source}`);
  }
}

/**
 * {@link assertValidMountPath} reused as both the verdict and the wording of a
 * schema check, so the five-rung ladder is stated once.
 */
function mountPathFailure(value: unknown, field: string): string | null {
  try {
    assertValidMountPath(value as string, field);
    return null;
  } catch (error) {
    return (error as Error).message;
  }
}

/** An optional absolute container directory, reported under `field`. */
function mountPath(field: string) {
  return z.optional(
    z.string({ error: `${field}, when provided, must be a string` }).check(
      z.refine((value: string) => mountPathFailure(value, field) === null, {
        error: (issue) => mountPathFailure(issue.input, field) ?? "",
        abort: true
      })
    )
  );
}

/**
 * Wire shape of any `kind: "asset"` ref (file / tool bundle), mounted at `path`.
 *
 * Declaration order is the order the checks report in, matching the sequential
 * assetId/name/mountPath ladder this replaces. Built per path rather than
 * memoised because callers mount it at indexed positions.
 */
export function assetRefSchema(path: string) {
  const assetId = `${path}.assetId must match ${ASSET_ID_PATTERN.source}`;
  const name = `${path}.name must be a non-empty string (<= 128 chars)`;
  return wireObject(
    path,
    {
      // Optional, because the normaliser stamps `kind: "asset"` on the way out
      // and a caller that omits it is complete. Not unread, though: `AssetRef`
      // declares `kind: "asset"` and nothing else, so a ref labelled anything
      // else is a caller mistake and is now told so rather than silently
      // relabelled.
      kind: z.optional(
        z.literal("asset", { error: `${path}.kind, when provided, must be "asset"` })
      ),
      assetId: z.string({ error: assetId }).check(z.regex(ASSET_ID_PATTERN, { error: assetId })),
      name: z
        .string({ error: name })
        .check(z.minLength(1, { error: name }), z.maxLength(128, { error: name })),
      mountPath: mountPath(`${path}.mountPath`)
    },
    {
      unknownKey: (objectPath, key) =>
        `${objectPath} contains unexpected field for asset ref: ${key}`
    }
  );
}

export type AssetRefWire = z.infer<ReturnType<typeof assetRefSchema>>;

/** Validate an asset ref mounted at `path`, in the family's own words. */
export function parseAssetRefWire(record: unknown, path: string): AssetRefWire {
  return parseWire(assetRefSchema(path), record);
}

/**
 * Stamp the discriminator and drop an absent `mountPath`.
 *
 * Separate from the schema per D4: `z.toJSONSchema(s, {io:"output"})` throws on
 * any `.transform()`, which would make the response half of the generated spec
 * ungenerable. Schemas validate; `normalize*()` functions transform.
 */
export function normalizeAssetRef(wire: AssetRefWire): AssetRef {
  return {
    kind: "asset",
    assetId: wire.assetId,
    name: wire.name,
    ...(wire.mountPath !== undefined ? { mountPath: wire.mountPath } : {})
  };
}
