/**
 * Response schemas for the versioned `workspace.*` resource families and for the
 * GDPR workspace erase.
 *
 * The three ASSET-BACKED kinds share one server projection (`publicResource`)
 * plus a per-kind metadata spread, so they are built here from one shared shape
 * rather than written out three times. The per-kind extras are exactly the
 * whitelist the publish path stores: `mountPath` for a file, `description` for
 * a skill, `description` + `entry` + `input_schema` for a tool. The
 * `instruction` kind is published as text and shares nothing — see the note on
 * its schema below.
 *
 * Divergence from the declared types: `WorkspaceResourceRecordFields` declares
 * `updatedAt?`, and the server projection never emits one. It stays optional
 * here — our own published type permits it, so a deployment that starts sending
 * it is not drift.
 */
import * as z from "zod/mini";
import {
  describeResponse,
  responseObject,
  wireLiteral,
  wireNonEmptyString,
  wireNonNegativeInteger,
  wirePositiveInteger,
  wireString
} from "./response-common.js";

const optional = z.optional;

/** The nine keys every ASSET-BACKED workspace resource record carries. */
const commonResourceShape = {
  resourceId: wireNonEmptyString,
  name: wireNonEmptyString,
  version: wirePositiveInteger,
  assetId: wireNonEmptyString,
  contentHash: wireNonEmptyString,
  sizeBytes: wireNonNegativeInteger,
  contentType: wireString,
  createdAt: wireNonEmptyString,
  updatedAt: optional(wireString)
} as const;

export const WorkspaceFileRecordSchema = describeResponse(
  "WorkspaceFileRecord",
  "An immutable published workspace file version, addressed by asset bytes.",
  responseObject({
    kind: wireLiteral("file"),
    ...commonResourceShape,
    mountPath: wireNonEmptyString
  })
);

export const WorkspaceSkillRecordSchema = describeResponse(
  "WorkspaceSkillRecord",
  "An immutable published workspace skill version.",
  responseObject({
    kind: wireLiteral("skill"),
    ...commonResourceShape,
    description: wireNonEmptyString
  })
);

export const WorkspaceToolRecordSchema = describeResponse(
  "WorkspaceToolRecord",
  "An immutable published workspace tool version. `input_schema` is the " +
    "provider-visible JSON Schema object, carried opaquely.",
  responseObject({
    kind: wireLiteral("tool"),
    ...commonResourceShape,
    description: wireNonEmptyString,
    entry: wireNonEmptyString,
    input_schema: z.record(wireString, z.unknown())
  })
);

/**
 * The one kind that is NOT asset-backed, and so the one kind that cannot share
 * {@link commonResourceShape}.
 *
 * An instruction is published as text. `assetId`, `contentHash` and
 * `contentType` name bytes in the content-addressed asset store and there are
 * none; `textHash` pins the trimmed UTF-8 text instead, and `sizeBytes` is that
 * text's byte length rather than a compressed archive size. The divergence from
 * the other three records is the honest encoding of that difference — see
 * references/modular-open-source-2026-07-27/11-archive-registration-redesign.md.
 */
export const WorkspaceInstructionRecordSchema = describeResponse(
  "WorkspaceInstructionRecord",
  "An immutable published workspace instruction version, pinned to its text rather than to asset bytes.",
  responseObject({
    kind: wireLiteral("instruction"),
    resourceId: wireNonEmptyString,
    name: wireNonEmptyString,
    version: wirePositiveInteger,
    textHash: wireNonEmptyString,
    sizeBytes: wireNonNegativeInteger,
    createdAt: wireNonEmptyString,
    updatedAt: optional(wireString)
  })
);

function singleResource<Schema extends z.core.$ZodType>(record: Schema) {
  return responseObject({ resource: record });
}

function resourcePage<Schema extends z.core.$ZodType>(record: Schema) {
  return responseObject({
    resources: z.array(record),
    nextCursor: optional(wireNonEmptyString)
  });
}

export const WorkspaceFileResponseSchema = describeResponse(
  "WorkspaceFileResponse",
  "One published workspace file version.",
  singleResource(WorkspaceFileRecordSchema)
);
export const WorkspaceSkillResponseSchema = describeResponse(
  "WorkspaceSkillResponse",
  "One published workspace skill version.",
  singleResource(WorkspaceSkillRecordSchema)
);
export const WorkspaceToolResponseSchema = describeResponse(
  "WorkspaceToolResponse",
  "One published workspace tool version.",
  singleResource(WorkspaceToolRecordSchema)
);
export const WorkspaceInstructionResponseSchema = describeResponse(
  "WorkspaceInstructionResponse",
  "One published workspace instruction version.",
  singleResource(WorkspaceInstructionRecordSchema)
);

export const WorkspaceFilePageResponseSchema = describeResponse(
  "WorkspaceFilePageResponse",
  "One page of workspace files, ordered by name. `nextCursor` is omitted on the last page.",
  resourcePage(WorkspaceFileRecordSchema)
);
export const WorkspaceSkillPageResponseSchema = describeResponse(
  "WorkspaceSkillPageResponse",
  "One page of workspace skills, ordered by name.",
  resourcePage(WorkspaceSkillRecordSchema)
);
export const WorkspaceToolPageResponseSchema = describeResponse(
  "WorkspaceToolPageResponse",
  "One page of workspace tools, ordered by name.",
  resourcePage(WorkspaceToolRecordSchema)
);
export const WorkspaceInstructionPageResponseSchema = describeResponse(
  "WorkspaceInstructionPageResponse",
  "One page of workspace instructions, ordered by name.",
  resourcePage(WorkspaceInstructionRecordSchema)
);

/**
 * `DELETE /workspaces/{workspaceId}` — the owner's GDPR hard-erase.
 *
 * A 200 with counters, not a 204, and idempotent: erasing an absent workspace
 * answers 200 with every counter at zero, indistinguishable from erasing an
 * empty one.
 *
 * This response had no declared client type — `deleteWorkspace()` in
 * `operations.ts` targets the CONTROL plane's `DELETE /api/workspaces/{id}` and
 * returns `void`. Same method, same path pattern, different plane; see the
 * plane-collision note in `testing/response-bindings.ts`. `WorkspaceEraseResult`
 * in `runtime-types.ts` is now `z.infer`red from the schema below, and names the
 * distinction so the two routes are no longer one undifferentiated `void`.
 *
 * `workspaceId` here is the RAW id, not the public `wsp_<hex>` form `whoami` and
 * the MCP-server records carry.
 */
export const WorkspaceEraseResponseSchema = describeResponse(
  "WorkspaceEraseResponse",
  "Counters from a workspace hard-erase. Idempotent: all zero when nothing was there.",
  responseObject({
    ok: wireLiteral(true),
    workspaceId: wireNonEmptyString,
    erased: responseObject({
      workspaceId: wireNonEmptyString,
      sessionsErased: wireNonNegativeInteger,
      deletedObjects: wireNonNegativeInteger,
      deletedConnections: wireNonNegativeInteger,
      deletedEgressRows: wireNonNegativeInteger,
      deletedEgressPolicies: wireNonNegativeInteger
    })
  })
);
