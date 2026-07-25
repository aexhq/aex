/**
 * Shape gates for the canonical asset-bundle pipeline.
 *
 * These schemas cover the parts of bundling that are genuinely *shape*
 * questions — is this a files map, is this entry bytes, is this fidelity
 * metadata an object — and stop there. Three kinds of rule deliberately stay in
 * `asset-bundle.ts`:
 *
 * - **Graph rules.** Duplicate paths, an `exec` entry that names no regular
 *   file, a symlink colliding with a leaf, and the leaf-prefix walk are
 *   relations *between* entries. A schema sees one value at a time.
 * - **Accounting.** The cumulative decompressed budget fails on the entry that
 *   crosses it, so it is a fold over the traversal, not a per-value predicate.
 * - **Anything downstream of canonicalisation.** `parseSkillBundleEntry`
 *   normalises a path, and the symlink-target diagnostics quote the *canonical*
 *   path. Per D4 a schema never transforms, so those checks necessarily run
 *   after the normaliser rather than inside a schema.
 *
 * Every factory takes the `kind`/`source` prefix its caller already computed,
 * because the two bundle kinds word the same failure differently ("Skill files
 * map is required" against "Skill bundle exceeds …"), and the fidelity
 * diagnostics are reused verbatim by the restore-side splitter.
 *
 * Nothing here is strict: bundle inputs are open maps keyed by caller-chosen
 * paths, and fidelity metadata must tolerate keys a newer SDK adds.
 */
import * as z from "zod/mini";

/** Count the own enumerable keys of an already-validated map-shaped value. */
function keyCount(value: unknown): number {
  return Object.keys(value as object).length;
}

/**
 * The files map handed to a bundle factory: present, non-empty, within the
 * entry ceiling.
 *
 * Each check aborts, so the ladder reports the same single failure the
 * sequential guard clauses did — a missing map never also reports as empty.
 */
export function bundleFilesMapSchema(kind: string, source: string, maxFiles: number) {
  return z.unknown().check(
    z.refine((value: unknown) => Boolean(value) && typeof value === "object", {
      error: `${kind} files map is required`,
      abort: true
    }),
    z.refine((value: unknown) => keyCount(value) > 0, {
      error: `${kind} files map cannot be empty`,
      abort: true
    }),
    z.refine((value: unknown) => keyCount(value) <= maxFiles, {
      error: (issue) => `${source} exceeds ${maxFiles} file limit (got ${keyCount(issue.input)})`,
      abort: true
    })
  );
}

/**
 * One authored file's contents, before the caller encodes a string to bytes.
 *
 * The encode stays outside: it is a transform, and the original guard tested
 * the coerced value only to catch what was neither a string nor bytes to begin
 * with, which is exactly this union.
 */
export function bundleFileContentsSchema(kind: string, rawPath: string) {
  return z.union([z.string(), z.instanceof(Uint8Array)], {
    error: `${kind} file "${rawPath}" must be a string or Uint8Array`
  });
}

/** One entry of an already-unzipped archive, which is bytes and never a string. */
export function archiveFileBytesSchema(source: string, rawPath: string) {
  return z.instanceof(Uint8Array, {
    error: `${source} file ${JSON.stringify(rawPath)} must be a Uint8Array`
  });
}

/**
 * The fidelity metadata envelope: an object whose `exec` and `symlinks`, when
 * present, are arrays.
 *
 * Element shape is deliberately `unknown` here. The per-element rules run
 * *after* the entry-count ceilings that sit between this gate and the traversal,
 * and reporting an element failure ahead of a count failure would invert the
 * order the guard clauses established.
 */
export function bundleFidelityMetaSchema(source: string) {
  return z.object(
    {
      exec: z.optional(
        z.array(z.unknown(), { error: `${source} fidelity metadata exec must be an array` })
      ),
      symlinks: z.optional(
        z.array(z.unknown(), { error: `${source} fidelity metadata symlinks must be an array` })
      )
    },
    { error: `${source} fidelity metadata must be an object` }
  );
}

/**
 * One `symlinks` element, checked for object-ness only.
 *
 * Its `path` and `target` are validated by the two schemas below, in the order
 * canonicalisation forces: the target diagnostics quote the canonical path, so
 * the path must be normalised before the target can be described.
 */
export function bundleSymlinkRecordSchema(source: string) {
  return z.object({}, { error: `${source} fidelity metadata contains a malformed symlink` });
}

/** A fidelity metadata path as supplied, before canonicalisation. */
export function bundleMetadataPathSchema(source: string, kind: "executable" | "symlink") {
  const message = `${source} ${kind} path must be a non-empty string`;
  return z.string({ error: message }).check(z.minLength(1, { error: message, abort: true }));
}

/** A captured symlink target, reported against the canonical link path. */
export function bundleSymlinkTargetSchema(source: string, path: string, maxLength: number) {
  return z
    .string({ error: `${source} symlink ${JSON.stringify(path)} target must be a string` })
    .check(
      z.maxLength(maxLength, {
        error: `${source} symlink ${JSON.stringify(path)} target exceeds ${maxLength} characters`,
        abort: true
      })
    );
}
