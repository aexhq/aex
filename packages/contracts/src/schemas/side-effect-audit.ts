/**
 * Schemas for the side-effect audit metadata envelope.
 *
 * Audit records are published to consumers that must never see request bodies,
 * raw URLs, or identity, so the envelope is a closed set of containers rather
 * than free-form metadata. These schemas are that closed set: the shape of each
 * one *is* the supported field list it rejects against.
 *
 * Two rules keep the schemas honest about what they do not cover:
 *
 * - **The public-safety scan stays outside.** It raises
 *   `SideEffectAuditRedactionError`, a typed error carrying `findings` that
 *   callers catch by class, which a schema cannot produce. `../side-effect-audit.ts`
 *   scans the whole payload before parsing it and again after building it.
 * - **`counts` and `timestamps` are described as opaque here.** Their keys come
 *   from `SIDE_EFFECT_AUDIT_COUNT_NAMES` / `…_TIMESTAMP_NAMES`, which are
 *   already the single source of that list; importing them would close an
 *   import cycle, and their entries are still checked by the normalisers that
 *   own those lists.
 */
import * as z from "zod/mini";
import { wireObject, type WireObjectText } from "./wire.js";

/**
 * This family words both of its own diagnostics the same way — an unsupported
 * container key and an unsupported nested key read identically, which is what
 * lets one text object serve every level of the envelope.
 */
const auditText: WireObjectText = {
  notObject: (path) => `side-effect audit ${path} must be an object`,
  unknownKey: (path, key) => `side-effect audit ${path}.${key} is not supported`
};

/**
 * A metadata string. Emptiness is NOT checked here: a falsy value is dropped by
 * the normaliser rather than rejected, and this schema must not turn that drop
 * into a failure. The "non-empty" wording is retained because it is the message
 * `assertSafeMetadataString` raises for the whitespace-only case that survives
 * to the normaliser.
 */
function metadataString(path: string) {
  return z.string({ error: `side-effect audit ${path} must be a non-empty string` });
}

/** A non-negative safe integer, worded as this family words it. */
function nonNegativeSafeInt(path: string) {
  const message = `side-effect audit ${path} must be a non-negative safe integer`;
  return z
    .number({ error: message })
    .check(
      z.refine((value: number) => Number.isSafeInteger(value) && value >= 0, {
        error: message,
        abort: true
      })
    );
}

/**
 * A closed value set, reported in the same `<what> <value> is not supported`
 * form the action, outcome, and principal-type checks already use.
 */
function auditEnum<const Values extends readonly string[]>(path: string, values: Values) {
  return z.enum(values, {
    error: (issue) => `side-effect audit ${path} ${String(issue.input)} is not supported`
  });
}

/** Wire shape of `metadata.status`. */
export const SideEffectAuditStatusMetadataSchema = wireObject(
  "metadata.status",
  {
    status: z.optional(metadataString("metadata.status.status")),
    statusCode: z.optional(nonNegativeSafeInt("metadata.status.statusCode")),
    errorClass: z.optional(metadataString("metadata.status.errorClass")),
    denialReason: z.optional(metadataString("metadata.status.denialReason")),
    followUpRequired: z.optional(
      z.boolean({ error: "side-effect audit metadata.status.followUpRequired must be a boolean" })
    )
  },
  auditText
);

/** Wire shape of `metadata.dimensions`. */
export const SideEffectAuditDimensionsMetadataSchema = wireObject(
  "metadata.dimensions",
  {
    provider: z.optional(metadataString("metadata.dimensions.provider")),
    namespace: z.optional(
      auditEnum("metadata.dimensions.namespace", [
        "metadata",
        "events",
        "logs",
        "files",
        "archive"
      ])
    ),
    method: z.optional(
      auditEnum("metadata.dimensions.method", ["GET", "POST", "PUT", "PATCH", "DELETE"])
    ),
    surface: z.optional(metadataString("metadata.dimensions.surface"))
  },
  auditText
);

/** The containers every audit action may carry. */
const undimensionedMetadataShape = {
  status: z.optional(SideEffectAuditStatusMetadataSchema),
  counts: z.optional(z.unknown()),
  timestamps: z.optional(z.unknown())
};

/**
 * Wire shape of `metadata` on a deletion action.
 *
 * Deletion audits are the record of data going away and are read after the
 * session they describe no longer exists, so they carry no `dimensions`: the
 * provider, namespace, method, and surface of the call are exactly the detail a
 * deletion record must not preserve. The full envelope below adds that
 * container back rather than either shape restating the shared three.
 */
export const SideEffectAuditDeletionMetadataSchema = wireObject(
  "metadata",
  undimensionedMetadataShape,
  auditText
);

/** Wire shape of `metadata` on every other action. */
export const SideEffectAuditMetadataSchema = wireObject(
  "metadata",
  {
    ...undimensionedMetadataShape,
    dimensions: z.optional(SideEffectAuditDimensionsMetadataSchema)
  },
  auditText
);
