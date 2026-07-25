/**
 * Schemas for the `.aexmeta.json` bundle fidelity sidecar.
 *
 * Two schemas, because the sidecar is read under different rules than it is
 * written:
 *
 * - {@link bundleManifestSidecarSchema} gates bytes that arrived from an
 *   untrusted zip. It carries **no messages**: `tryParseBundleManifest` reports
 *   a rejected sidecar as `null`, never as a thrown diagnostic, and a message no
 *   caller can observe is a message that will drift.
 * - {@link serializableBundleManifestSchema} gates a manifest on its way out and
 *   *does* throw, so it states exactly the two limits the serializer already
 *   enforced and nothing more. It deliberately does not re-validate the shape:
 *   its input is `BundleManifest`, and widening an output guard into a full
 *   structural check would reject values the type system already admitted.
 *
 * Both are factories over their limits rather than importers of them, so
 * `bundle-manifest.ts` stays the single site that decides what the caps are and
 * this module stays free of a cycle back into it.
 *
 * Neither schema is strict. An unknown key in a sidecar written by a newer SDK
 * must be ignored, not rejected — that forward-compatibility is the whole point
 * of the `v` field.
 */
import * as z from "zod/mini";

/**
 * A v1 sidecar: `exec` paths and `symlinks` leaves, each array bounded before
 * the graph that consumes it is built.
 *
 * An unknown `v` fails the literal and therefore yields `null`, which is the
 * forward-compatible outcome: metadata is dropped and the restore proceeds
 * content-only.
 */
export function bundleManifestSidecarSchema(maxRecords: number, maxSymlinkTargetLength: number) {
  return z.object({
    v: z.literal(1),
    exec: z.array(z.string().check(z.minLength(1))).check(z.maxLength(maxRecords)),
    symlinks: z
      .array(
        z.object({
          path: z.string().check(z.minLength(1)),
          target: z.string().check(z.maxLength(maxSymlinkTargetLength))
        })
      )
      .check(z.maxLength(maxRecords))
  });
}

export type BundleManifestSidecarSchema = ReturnType<typeof bundleManifestSidecarSchema>;
export type BundleManifestSidecar = z.infer<BundleManifestSidecarSchema>;

/**
 * Parse a decoded sidecar value, collapsing every rejection to `null`.
 *
 * The `null`-returning counterpart to `parseWire`. It lives here rather than in
 * `wire.ts` because a nullable outcome is this family's convention, not the
 * package's: every other wire parser throws.
 */
export function tryParseBundleManifestSidecar(
  schema: BundleManifestSidecarSchema,
  value: unknown
): BundleManifestSidecar | null {
  const result = z.safeParse(schema, value);
  return result.success ? result.data : null;
}

/**
 * The two limits `serializeBundleManifest` refuses to encode past.
 *
 * Both arrays share the record-limit wording, and `exec` is declared first, so
 * the shallowest-issue rule in `errorFromZod` reproduces the original
 * `exec.length > max || symlinks.length > max` short-circuit. A too-long target
 * sits two levels deeper and therefore still loses to either record limit,
 * matching the order the guard clauses ran in.
 */
export function serializableBundleManifestSchema(
  maxRecords: number,
  maxSymlinkTargetLength: number
) {
  const recordLimit = `bundle fidelity metadata exceeds the ${maxRecords}-record limit`;
  const targetLimit = `bundle fidelity symlink target exceeds ${maxSymlinkTargetLength} characters`;
  return z.object({
    exec: z.array(z.unknown()).check(z.maxLength(maxRecords, { error: recordLimit })),
    symlinks: z
      .array(
        z.object({
          target: z.string().check(z.maxLength(maxSymlinkTargetLength, { error: targetLimit }))
        })
      )
      .check(z.maxLength(maxRecords, { error: recordLimit }))
  });
}
