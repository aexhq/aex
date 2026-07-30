/**
 * The prelaunch v1 public resource contract.
 *
 * These strict Standard Schemas are the wire authority shared by clients,
 * hosted handlers, conformance tests, and generated OpenAPI. They deliberately
 * do not accept the retired runtime/checkpoint/session-management model.
 */
import * as z from "zod/mini";
import {
  WORKSPACE_API_KEY_PATTERN,
  isWorkspaceApiKeyValue
} from "./api-key.js";
import { isId, type Id, type IdKind } from "./ids.js";

const timestamp = z.string().check(
  z.regex(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/)
);
const nonEmptyString = z.string().check(z.minLength(1));
const positiveInteger = z.int().check(z.gte(1));
const nonNegativeInteger = z.int().check(z.gte(0));
const sha256 = z.string().check(z.regex(/^sha256:[0-9a-f]{64}$/));

function resourceId<K extends IdKind>(kind: K) {
  return z.string().check(z.refine((value): value is Id<K> => isId(kind, value)));
}

export const ApiErrorBodySchema = z.strictObject({
  code: nonEmptyString,
  message: z.string(),
  requestId: nonEmptyString,
  retryable: z.boolean(),
  operationId: z.optional(resourceId("operation")),
  details: z.optional(z.record(z.string(), z.unknown()))
});

export const ApiErrorSchema = z.strictObject({
  error: ApiErrorBodySchema
});

export type ApiErrorBody = z.infer<typeof ApiErrorBodySchema>;
export type ApiError = z.infer<typeof ApiErrorSchema>;

export function PageSchema<Schema extends z.core.$ZodType>(item: Schema) {
  return z.strictObject({
    items: z.array(item),
    nextCursor: z.optional(z.string().check(z.regex(/^cur_/)))
  });
}

export type Page<T> = { readonly items: readonly T[]; readonly nextCursor?: string };

export const RegionSchema = z.enum([
  "us-east-1",
  "us-east-2",
  "us-west-2",
  "ap-northeast-1",
  "eu-west-1"
]);
export type Region = z.infer<typeof RegionSchema>;

/**
 * A one-time workspace API-key value.
 *
 * The 26-character middle field is the public `key_` UUIDv7 suffix used for
 * indexed lookup. The final field is exactly 32 CSPRNG bytes encoded as
 * canonical, unpadded base64url (43 characters). The service stores only a
 * password-hash/KDF result for that secret. There is no plane, workspace id,
 * checksum, or CRC field in the value.
 */
export const WorkspaceApiKeyValueSchema = z.string().check(
  z.regex(WORKSPACE_API_KEY_PATTERN),
  z.refine(isWorkspaceApiKeyValue)
);
export type WorkspaceApiKeyValue = z.infer<typeof WorkspaceApiKeyValueSchema>;

export const WorkspaceContinuitySchema = z.discriminatedUnion("state", [
  z.strictObject({
    state: z.literal("warm"),
    availability: z.enum(["running", "suspended"]),
    generationId: resourceId("generation"),
    materializedFromPersistRevision: nonNegativeInteger,
    changedAt: timestamp
  }),
  z.strictObject({
    state: z.literal("cold"),
    persistedRevision: nonNegativeInteger,
    changedAt: timestamp,
    reason: z.enum([
      "not_started",
      "idle_retention_elapsed",
      "hard_lifetime_reached",
      "user_discarded",
      "unexpected_loss"
    ]),
    previousGenerationId: z.optional(resourceId("generation"))
  })
]);
export type WorkspaceContinuity = z.infer<typeof WorkspaceContinuitySchema>;

export const ApprovalPolicySchema = z.discriminatedUnion("mode", [
  z.strictObject({ mode: z.literal("allow_all") }),
  z.strictObject({
    mode: z.literal("require_for_tools"),
    tools: z.array(nonEmptyString).check(z.minLength(1))
  })
]);

export const ComputeSizeSchema = z.enum(["512mb", "1gb", "2gb", "4gb", "8gb"]);
export const DiskSizeSchema = z.enum(["8gb", "16gb", "32gb"]);
export const PackageEcosystemSchema = z.enum(["apt", "pip", "npm"]);

export const PackageRequestSchema = z.strictObject({
  ecosystem: PackageEcosystemSchema,
  name: nonEmptyString,
  version: nonEmptyString
});

const networkSchema = z.strictObject({
  hands: z.strictObject({ mode: z.enum(["none", "public_internet"]) })
});

export const SessionCreateRequestSchema = z.strictObject({
  model: nonEmptyString,
  registered: z.optional(z.strictObject({
    files: z.optional(z.array(nonEmptyString)),
    skills: z.optional(z.array(nonEmptyString)),
    tools: z.optional(z.array(nonEmptyString)),
    instructions: z.optional(z.array(nonEmptyString)),
    mcpServers: z.optional(z.array(nonEmptyString))
  })),
  credentials: z.optional(z.strictObject({
    secrets: z.array(z.strictObject({ name: nonEmptyString }))
  })),
  compute: z.optional(z.strictObject({
    requestedSize: z.optional(ComputeSizeSchema),
    peakSize: z.optional(ComputeSizeSchema),
    diskSize: z.optional(DiskSizeSchema)
  })),
  network: z.optional(networkSchema),
  packages: z.optional(z.array(PackageRequestSchema)),
  approvalPolicy: z.optional(ApprovalPolicySchema),
  metadata: z.optional(z.record(
    z.string(),
    z.union([z.string(), z.number(), z.boolean(), z.null()])
  ))
});
export type SessionCreateRequestV1 = z.infer<typeof SessionCreateRequestSchema>;

const builtinTool = z.enum([
  "bash",
  "read_file",
  "write_file",
  "edit_file",
  "grep",
  "glob",
  "head",
  "tail",
  "todo_write",
  "subagent",
  "subagent_result",
  "web_fetch",
  "web_search",
  "bash_output",
  "bash_kill",
  "code_execution",
  "wait",
  "git",
  "ls",
  "stat",
  "wc"
]);

export const ResolvedSessionConfigSchema = z.strictObject({
  builtinCatalogHash: sha256,
  builtinTools: z.array(builtinTool),
  approvalPolicy: ApprovalPolicySchema,
  network: networkSchema,
  packages: z.array(PackageRequestSchema),
  compute: z.strictObject({
    requestedSize: ComputeSizeSchema,
    peakSize: ComputeSizeSchema,
    diskSize: DiskSizeSchema
  }),
  continuityPolicy: z.strictObject({
    idleAction: z.literal("hibernate"),
    idleDelayMs: z.literal(180_000),
    warmRetention: z.literal("provider_lifetime"),
    hardLifetimeMs: z.literal(28_800_000)
  }),
  harness: z.strictObject({
    protocol: z.literal("aex-agent-v1"),
    revision: sha256,
    platformPrompt: z.literal("required"),
    instructionDiscovery: z.literal("none"),
    toolResultContextLimitBytes: z.literal(65_536)
  })
});

export const SessionStatusSchema = z.enum([
  "idle",
  "running",
  "awaiting_approval",
  "deleting"
]);
export type SessionStatusV1 = z.infer<typeof SessionStatusSchema>;

export const SessionSchema = z.strictObject({
  id: resourceId("session"),
  workspaceId: resourceId("workspace"),
  status: SessionStatusSchema,
  revision: positiveInteger,
  persistRevision: nonNegativeInteger,
  createdAt: timestamp,
  updatedAt: timestamp,
  lastPersistedAt: z.optional(timestamp),
  continuity: WorkspaceContinuitySchema,
  lineage: z.strictObject({
    parentSessionId: z.optional(resourceId("session")),
    forkedAt: z.optional(timestamp)
  }),
  resolvedConfig: ResolvedSessionConfigSchema
});
export type SessionV1 = z.infer<typeof SessionSchema>;

export const MessagePartSchema = z.discriminatedUnion("type", [
  z.strictObject({ type: z.literal("text"), text: nonEmptyString }),
  z.strictObject({
    type: z.literal("file"),
    path: nonEmptyString,
    source: z.literal("persisted"),
    mediaType: z.optional(nonEmptyString)
  })
]);

export const MessageSendRequestSchema = z.strictObject({
  content: z.array(MessagePartSchema).check(z.minLength(1)),
  maxSpendCents: z.optional(positiveInteger)
});
export type MessageSendRequest = z.infer<typeof MessageSendRequestSchema>;

export const MessageSchema = z.strictObject({
  id: resourceId("message"),
  sessionId: resourceId("session"),
  runId: z.optional(resourceId("run")),
  role: z.enum(["user", "assistant", "tool"]),
  content: z.array(MessagePartSchema),
  createdAt: timestamp
});
export type MessageV1 = z.infer<typeof MessageSchema>;

export const RunStatusSchema = z.enum([
  "queued",
  "running",
  "succeeded",
  "failed",
  "timed_out",
  "cancelled",
  "interrupted"
]);
export type RunStatusV1 = z.infer<typeof RunStatusSchema>;

export const RunSchema = z.strictObject({
  id: resourceId("run"),
  sessionId: resourceId("session"),
  messageId: resourceId("message"),
  status: RunStatusSchema,
  maxSpendCents: positiveInteger,
  queuedAt: timestamp,
  startedAt: z.optional(timestamp),
  terminalAt: z.optional(timestamp),
  outputMessageIds: z.optional(z.array(resourceId("message"))),
  error: z.optional(ApiErrorBodySchema),
  telemetryComplete: z.optional(z.boolean()),
  telemetryRejectionIds: z.optional(z.array(nonEmptyString))
});
export type RunV1 = z.infer<typeof RunSchema>;

const residualRetention = z.strictObject({
  system: nonEmptyString,
  class: z.enum(["managed_backup", "stream", "provider_media"]),
  state: z.enum(["scheduled", "verified", "unknown"]),
  purgeBy: z.optional(timestamp)
});

export const SessionTombstoneSchema = z.strictObject({
  sessionId: resourceId("session"),
  workspaceId: resourceId("workspace"),
  deletedAt: timestamp,
  deletionOperationId: resourceId("operation"),
  residualRetention: z.array(residualRetention)
});

export const WorkspaceTombstoneSchema = z.strictObject({
  workspaceId: resourceId("workspace"),
  deletedAt: timestamp,
  deletionOperationId: resourceId("operation"),
  residualRetention: z.array(residualRetention)
});

const stopResult = z.strictObject({
  sessionId: resourceId("session"),
  changed: z.boolean(),
  sessionRevision: positiveInteger
});
const persistResult = z.strictObject({
  sessionId: resourceId("session"),
  changed: z.boolean(),
  persistRevision: nonNegativeInteger,
  rootHash: sha256,
  lastPersistedAt: timestamp,
  added: nonNegativeInteger,
  updated: nonNegativeInteger,
  deleted: nonNegativeInteger,
  bytesMoved: nonNegativeInteger
});
const forkResult = z.strictObject({ session: SessionSchema });
const discardResult = z.strictObject({
  sessionId: resourceId("session"),
  changed: z.boolean(),
  continuity: WorkspaceContinuitySchema
});
const rebindResult = z.strictObject({
  sessionId: resourceId("session"),
  custodyRevision: positiveInteger,
  secrets: z.array(z.strictObject({ name: nonEmptyString }))
});
const exportResult = z.strictObject({
  exportId: resourceId("export"),
  format: z.enum(["ndjson", "parquet", "otlp_json"]),
  manifestHash: sha256,
  expiresAt: timestamp
});

const operationBase = {
  id: resourceId("operation"),
  workspaceId: resourceId("workspace"),
  sessionId: z.optional(resourceId("session")),
  status: z.enum(["queued", "running", "succeeded", "failed", "cancelled"]),
  progress: z.optional(z.strictObject({
    phase: nonEmptyString,
    completed: z.optional(nonNegativeInteger),
    total: z.optional(nonNegativeInteger)
  })),
  cancelable: z.boolean(),
  error: z.optional(ApiErrorBodySchema),
  createdAt: timestamp,
  startedAt: z.optional(timestamp),
  updatedAt: timestamp,
  committedAt: z.optional(timestamp),
  terminalAt: z.optional(timestamp)
} as const;

function operationVariant<
  Kind extends string,
  Result extends z.core.$ZodType
>(kind: Kind, result: Result) {
  return z.strictObject({
    ...operationBase,
    kind: z.literal(kind),
    result: z.optional(result)
  });
}

export const OperationSchema = z.discriminatedUnion("kind", [
  operationVariant("session_stop", stopResult),
  operationVariant("session_persist", persistResult),
  operationVariant("session_fork", forkResult),
  operationVariant("workspace_discard", discardResult),
  operationVariant("session_delete", SessionTombstoneSchema),
  operationVariant("credential_rebind", rebindResult),
  operationVariant("telemetry_export", exportResult),
  operationVariant("workspace_delete", WorkspaceTombstoneSchema)
]);

export type Operation = z.infer<typeof OperationSchema>;
export type OperationKind = Operation["kind"];
export type OperationStatusV1 = Operation["status"];
