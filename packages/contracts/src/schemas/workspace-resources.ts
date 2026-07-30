/**
 * Schemas for persisted workspace resource references.
 *
 * Two independent gates live here:
 *
 * - {@link workspaceResourceNameSchema} — the directly-supplied resource *name*
 *   grammar. Validation only: an accepted name is stored verbatim, so there is
 *   deliberately no normaliser to pair with it (no trim, no case folding).
 * - {@link pinnedWorkspaceResourceSchema} — the immutable identity quartet every
 *   submitted resource carries, plus the `assetId`/`contentHash` agreement.
 *
 * **Both are factories over the wire grammars rather than owners of them.** The
 * name patterns are supplied because there are two of them with deliberately
 * separate identities (`workspace-resources.ts`), and the digest pattern is
 * supplied because `canonical-sha256.ts` is the single declaring site for that
 * grammar and `workspace-resources.ts` is its declared consumer — an invariant
 * `canonical-sha256-ownership.test.ts` enforces. Threading them in keeps this
 * module about *shape* and leaves grammar ownership where it already is.
 *
 * Neither schema is strict. `assertPinnedWorkspaceResource` is handed a fully
 * projected `WorkspaceResourceRef` — `kind`, `name`, `mountPath`, and friends
 * ride along — and rejecting those would break every caller. Unknown-key
 * rejection for the resource objects themselves is the submission parser's job,
 * one level up, where the per-kind key list actually exists.
 */
import * as z from "zod/mini";
import { idPattern } from "../ids.js";

/**
 * Opaque logical resource id minted by the platform.
 *
 * Sourced from the id owner rather than re-declared: `scripts/validate/id-format-parity.test.ts`
 * fails on any `^<prefix>_` literal outside `ids.ts`, because eight hand-copies of the
 * workspace-id pattern had already drifted (one lacked the `/i` flag the others carried,
 * so the same id passed everywhere and threw at the telemetry boundary).
 */
const WORKSPACE_RESOURCE_ID_PATTERN = idPattern("upload");

/** Bytes prefix stripped from a canonical digest to derive the asset id. */
const DIGEST_PREFIX = "sha256:";

/**
 * A directly supplied resource name: matches `pattern`, and never contains the
 * reserved `__` separator.
 *
 * The envelope failure and the type failure share one message because the
 * ladder this replaces collapsed them (`typeof value !== "string" || !pattern.test(value)`),
 * and `abort` keeps the separator complaint from firing on a name that never
 * cleared the envelope.
 */
export function workspaceResourceNameSchema(path: string, pattern: RegExp) {
  const envelope = `${path} must match ${pattern.source}`;
  return z.string({ error: envelope }).check(
    z.refine((value: string) => pattern.test(value), { error: envelope, abort: true }),
    z.refine((value: string) => !value.includes("__"), {
      error: `${path} must not contain "__"`,
      abort: true
    })
  );
}

/**
 * The two identity fields every workspace resource carries, whatever it is
 * pinned TO. Split out because the `instruction` kind is pinned to text and the
 * other three to content-addressed bytes, so only these two are shared.
 */
function logicalResourceKeys(path: string) {
  const resourceId = `${path}.resourceId must match wres_<32 lowercase hex>`;
  const version = `${path}.version must be a positive integer`;
  return {
    resourceId: z.string({ error: resourceId }).check(
      z.refine((value: string) => WORKSPACE_RESOURCE_ID_PATTERN.test(value), {
        error: resourceId,
        abort: true
      })
    ),
    version: z.number({ error: version }).check(
      z.refine((value: number) => Number.isSafeInteger(value) && value >= 1, {
        error: version,
        abort: true
      })
    )
  } as const;
}

/**
 * The immutable identity of a pinned INSTRUCTION: the logical resource version
 * plus a digest of its text.
 *
 * There is no asset agreement check to run here — that check exists because an
 * `assetId` and a `contentHash` are two names for the same bytes and can
 * disagree. A `textHash` has no second name.
 */
export function pinnedWorkspaceInstructionSchema(path: string, digestPattern: RegExp) {
  const textHash = `${path}.textHash must be a sha256 digest`;
  return z.object({
    ...logicalResourceKeys(path),
    textHash: z.string({ error: textHash }).check(
      z.refine((value: string) => digestPattern.test(value), {
        error: textHash,
        abort: true
      })
    )
  });
}

/**
 * The immutable identity fields common to every ASSET-BACKED submitted
 * workspace resource.
 *
 * Field order is the order the sequential assertions this replaces ran in, and
 * `errorFromZod` reports the shallowest issue, so a field complaint still wins
 * over the `assetId`/`contentHash` agreement — which Zod only evaluates once
 * every field has parsed (measured), exactly as the original ladder did.
 */
export function pinnedWorkspaceResourceSchema(path: string, digestPattern: RegExp) {
  const assetId = `${path}.assetId must be a non-empty string`;
  const contentHash = `${path}.contentHash must be a sha256 digest`;
  return z
    .object({
      ...logicalResourceKeys(path),
      assetId: z
        .string({ error: assetId })
        .check(z.minLength(1, { error: assetId, abort: true })),
      contentHash: z.string({ error: contentHash }).check(
        z.refine((value: string) => digestPattern.test(value), {
          error: contentHash,
          abort: true
        })
      )
    })
    .check(
      z.refine(
        (value) => value.assetId === `asset_${value.contentHash.slice(DIGEST_PREFIX.length)}`,
        { error: `${path}.assetId must identify the same bytes as contentHash` }
      )
    );
}
