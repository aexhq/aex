/** Strict v1 contracts for files, registries, uploads, secrets, and approvals. */
import * as z from "zod/mini";
import { isId, type IdKind } from "./ids.js";
import { PageSchema } from "./v1-resources.js";

const timestamp = z.string().check(
  z.regex(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/)
);
const nonEmptyString = z.string().check(z.minLength(1));
const nonNegativeInteger = z.int().check(z.gte(0));
const positiveInteger = z.int().check(z.gte(1));
const sha256 = z.string().check(z.regex(/^sha256:[0-9a-f]{64}$/));

function resourceId<K extends IdKind>(kind: K) {
  return z.string().check(z.refine((value) => isId(kind, value)));
}

export const RegisteredNameSchema = z.string().check(
  z.regex(/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/)
);

/** AWS S3's provider-hard maximum selected byte range for one GetObject. */
export const MAX_SINGLE_GET_BYTES = 5_000_000_000_000;

export const ByteRangeSchema = z.strictObject({
  start: nonNegativeInteger,
  endExclusive: positiveInteger
}).check(
  z.refine((range) => range.endExclusive > range.start),
  z.refine(
    (range) => range.endExclusive - range.start <= MAX_SINGLE_GET_BYTES
  )
);
export type ByteRange = z.infer<typeof ByteRangeSchema>;

export const FileEntrySchema = z.strictObject({
  path: nonEmptyString,
  type: z.enum(["file", "directory", "symlink"]),
  sizeBytes: nonNegativeInteger,
  sha256: z.optional(sha256),
  mode: nonEmptyString,
  mtime: timestamp,
  target: z.optional(nonEmptyString)
});
export type FileEntry = z.infer<typeof FileEntrySchema>;

const fileListBase = {
  path: z.optional(nonEmptyString),
  recursive: z.optional(z.boolean()),
  limit: z.optional(positiveInteger),
  cursor: z.optional(z.string())
} as const;

const fileStatBase = { path: nonEmptyString } as const;
const liveAccess = {
  wake: z.optional(z.enum(["retained", "never"])),
  consistency: z.optional(z.enum(["coherent", "best_effort"])),
  ifGenerationId: z.optional(resourceId("generation"))
} as const;

export const PersistedFileListRequestSchema = z.strictObject(fileListBase);
export const LiveFileListRequestSchema = z.strictObject({
  ...fileListBase,
  ...liveAccess
});
export const PersistedFileStatRequestSchema = z.strictObject(fileStatBase);
export const LiveFileStatRequestSchema = z.strictObject({
  ...fileStatBase,
  ...liveAccess
});
export const FileDownloadRequestSchema = z.strictObject({
  path: nonEmptyString,
  range: z.optional(ByteRangeSchema)
});
export const LiveFileDownloadRequestSchema = z.strictObject({
  path: nonEmptyString,
  range: z.optional(ByteRangeSchema),
  ...liveAccess
});

export type PersistedFileListRequest = z.infer<typeof PersistedFileListRequestSchema>;
export type LiveFileListRequest = z.infer<typeof LiveFileListRequestSchema>;
export type PersistedFileStatRequest = z.infer<typeof PersistedFileStatRequestSchema>;
export type LiveFileStatRequest = z.infer<typeof LiveFileStatRequestSchema>;
export type FileDownloadRequest = z.infer<typeof FileDownloadRequestSchema>;
export type LiveFileDownloadRequest = z.infer<typeof LiveFileDownloadRequestSchema>;

export const WorkspaceAccessSchema = z.strictObject({
  generationId: resourceId("generation"),
  resumed: z.boolean()
});
export type WorkspaceAccess = z.infer<typeof WorkspaceAccessSchema>;

export const DownloadGrantSchema = z.strictObject({
  url: nonEmptyString,
  headers: z.optional(z.record(z.string(), z.string())),
  expiresAt: timestamp,
  sizeBytes: nonNegativeInteger,
  authorizedBytes: nonNegativeInteger,
  measurementId: resourceId("measurement"),
  sha256
}).check(z.refine((grant) => grant.authorizedBytes <= grant.sizeBytes));
export type DownloadGrant = z.infer<typeof DownloadGrantSchema>;

export const LiveDownloadGrantSchema = z.strictObject({
  url: nonEmptyString,
  headers: z.optional(z.record(z.string(), z.string())),
  expiresAt: timestamp,
  sizeBytes: nonNegativeInteger,
  authorizedBytes: nonNegativeInteger,
  measurementId: resourceId("measurement"),
  sha256,
  workspaceAccess: WorkspaceAccessSchema
}).check(z.refine((grant) => grant.authorizedBytes <= grant.sizeBytes));
export type LiveDownloadGrant = z.infer<typeof LiveDownloadGrantSchema>;

export const FilePageSchema = PageSchema(FileEntrySchema);
export const LiveFilePageSchema = z.strictObject({
  items: z.array(FileEntrySchema),
  nextCursor: z.optional(z.string().check(z.regex(/^cur_/))),
  workspaceAccess: WorkspaceAccessSchema
});
export type LiveFilePage = z.infer<typeof LiveFilePageSchema>;
export const LiveFileEntrySchema = z.strictObject({
  path: nonEmptyString,
  type: z.enum(["file", "directory", "symlink"]),
  sizeBytes: nonNegativeInteger,
  sha256: z.optional(sha256),
  mode: nonEmptyString,
  mtime: timestamp,
  target: z.optional(nonEmptyString),
  workspaceAccess: WorkspaceAccessSchema
});
export type LiveFileEntry = z.infer<typeof LiveFileEntrySchema>;

export const BlobInputSchema = z.discriminatedUnion("type", [
  z.strictObject({
    type: z.literal("inline"),
    encoding: z.enum(["utf8", "base64"]),
    data: z.string(),
    sha256
  }),
  z.strictObject({
    type: z.literal("upload"),
    uploadId: resourceId("upload"),
    sha256,
    sizeBytes: nonNegativeInteger
  })
]);
export type BlobInput = z.infer<typeof BlobInputSchema>;

export const BlobDescriptorSchema = z.strictObject({
  sha256,
  sizeBytes: nonNegativeInteger
});
export type BlobDescriptor = z.infer<typeof BlobDescriptorSchema>;

export const RegisteredFileInputSchema = z.strictObject({
  mountPath: nonEmptyString,
  content: BlobInputSchema,
  mediaType: nonEmptyString,
  mode: z.enum(["0644", "0755"])
});
export const RegisteredSkillInputSchema = z.strictObject({
  description: z.string(),
  bundleFormat: z.literal("tar.gz"),
  bundle: BlobInputSchema
});
export const RegisteredToolInputSchema = z.strictObject({
  description: z.string(),
  inputSchema: z.record(z.string(), z.unknown()),
  entry: nonEmptyString,
  bundleFormat: z.literal("tar.gz"),
  bundle: BlobInputSchema
});
export const RegisteredInstructionInputSchema = z.strictObject({
  text: z.string()
});
export const RegisteredMcpServerInputSchema = z.strictObject({
  url: nonEmptyString,
  transport: z.literal("streamable_http"),
  headers: z.array(z.strictObject({
    name: nonEmptyString,
    secretName: RegisteredNameSchema
  }))
});

export const RegisteredFileValueSchema = z.strictObject({
  mountPath: nonEmptyString,
  content: BlobDescriptorSchema,
  mediaType: nonEmptyString,
  mode: z.enum(["0644", "0755"])
});
export const RegisteredSkillValueSchema = z.strictObject({
  description: z.string(),
  bundleFormat: z.literal("tar.gz"),
  bundle: BlobDescriptorSchema
});
export const RegisteredToolValueSchema = z.strictObject({
  description: z.string(),
  inputSchema: z.record(z.string(), z.unknown()),
  entry: nonEmptyString,
  bundleFormat: z.literal("tar.gz"),
  bundle: BlobDescriptorSchema
});
export const RegisteredInstructionValueSchema = z.strictObject({
  text: z.string()
});
export const RegisteredMcpServerValueSchema = z.strictObject({
  url: nonEmptyString,
  transport: z.literal("streamable_http"),
  headers: z.array(z.strictObject({
    name: nonEmptyString,
    secretName: RegisteredNameSchema
  }))
});

export type RegisteredFileInput = z.infer<typeof RegisteredFileInputSchema>;
export type RegisteredSkillInput = z.infer<typeof RegisteredSkillInputSchema>;
export type RegisteredToolInput = z.infer<typeof RegisteredToolInputSchema>;
export type RegisteredInstructionInput = z.infer<typeof RegisteredInstructionInputSchema>;
export type RegisteredMcpServerInput = z.infer<typeof RegisteredMcpServerInputSchema>;
export type RegisteredFileValue = z.infer<typeof RegisteredFileValueSchema>;
export type RegisteredSkillValue = z.infer<typeof RegisteredSkillValueSchema>;
export type RegisteredToolValue = z.infer<typeof RegisteredToolValueSchema>;
export type RegisteredInstructionValue = z.infer<typeof RegisteredInstructionValueSchema>;
export type RegisteredMcpServerValue = z.infer<typeof RegisteredMcpServerValueSchema>;

const registeredBase = {
  name: RegisteredNameSchema,
  revision: positiveInteger,
  state: z.literal("current"),
  sha256,
  sizeBytes: nonNegativeInteger,
  createdAt: timestamp,
  updatedAt: timestamp
} as const;

function registeredVariant<Kind extends string, Value extends z.core.$ZodType>(
  kind: Kind,
  value: Value
) {
  return z.strictObject({ kind: z.literal(kind), ...registeredBase, value });
}

function registeredSummaryVariant<Kind extends string>(kind: Kind) {
  return z.strictObject({ kind: z.literal(kind), ...registeredBase });
}

export const RegisteredResourceSchema = z.discriminatedUnion("kind", [
  registeredVariant("file", RegisteredFileValueSchema),
  registeredVariant("skill", RegisteredSkillValueSchema),
  registeredVariant("tool", RegisteredToolValueSchema),
  registeredVariant("instruction", RegisteredInstructionValueSchema),
  registeredVariant("mcp_server", RegisteredMcpServerValueSchema)
]);
export const RegisteredResourceSummarySchema = z.discriminatedUnion("kind", [
  registeredSummaryVariant("file"),
  registeredSummaryVariant("skill"),
  registeredSummaryVariant("tool"),
  registeredSummaryVariant("instruction"),
  registeredSummaryVariant("mcp_server")
]);
export const RegisteredResourcePageSchema =
  PageSchema(RegisteredResourceSummarySchema);
export type RegisteredResource = z.infer<typeof RegisteredResourceSchema>;
export type RegisteredResourceSummary = z.infer<typeof RegisteredResourceSummarySchema>;
export type RegisteredResourcePage = z.infer<typeof RegisteredResourcePageSchema>;
export type RegisteredResourceKind = RegisteredResource["kind"];
export interface RegisteredResourceInputByKind {
  readonly file: RegisteredFileInput;
  readonly skill: RegisteredSkillInput;
  readonly tool: RegisteredToolInput;
  readonly instruction: RegisteredInstructionInput;
  readonly mcp_server: RegisteredMcpServerInput;
}
export type RegisteredResourceInput<K extends RegisteredResourceKind> =
  RegisteredResourceInputByKind[K];

export const RegistryPutResultSchema = z.strictObject({
  status: z.enum(["created", "replaced", "unchanged"]),
  resource: RegisteredResourceSchema
});
export type RegistryPutResult = z.infer<typeof RegistryPutResultSchema>;

export const RegisteredFileDownloadRequestSchema = z.strictObject({
  range: z.optional(ByteRangeSchema)
});
export type RegisteredFileDownloadRequest =
  z.infer<typeof RegisteredFileDownloadRequestSchema>;

export const UploadCreateRequestSchema = z.strictObject({
  sizeBytes: nonNegativeInteger,
  sha256,
  contentType: nonEmptyString
});
export const UploadPartsRequestSchema = z.strictObject({
  parts: z.array(z.strictObject({
    partNumber: positiveInteger,
    sizeBytes: positiveInteger,
    sha256
  })).check(
    z.minLength(1),
    z.refine((parts) => new Set(parts.map(({ partNumber }) => partNumber)).size === parts.length)
  )
});
export const UploadCompleteRequestSchema = z.strictObject({
  parts: z.array(z.strictObject({
    partNumber: positiveInteger,
    etag: nonEmptyString,
    sizeBytes: positiveInteger,
    sha256
  })).check(
    z.minLength(1),
    z.refine((parts) => parts.every(({ partNumber }, index) => partNumber === index + 1))
  )
});
export const UploadSchema = z.strictObject({
  id: resourceId("upload"),
  state: z.enum(["pending", "uploading", "ready", "aborted"]),
  sizeBytes: nonNegativeInteger,
  sha256,
  contentType: nonEmptyString,
  createdAt: timestamp,
  expiresAt: timestamp
});
export const UploadPartGrantSchema = z.strictObject({
  partNumber: positiveInteger,
  url: nonEmptyString,
  headers: z.optional(z.record(z.string(), z.string())),
  expiresAt: timestamp
});
export type UploadPartGrant = z.infer<typeof UploadPartGrantSchema>;
export const UploadPartsResponseSchema = z.strictObject({
  parts: z.array(UploadPartGrantSchema)
});
export type UploadCreateRequest = z.infer<typeof UploadCreateRequestSchema>;
export type UploadPartsRequest = z.infer<typeof UploadPartsRequestSchema>;
export type UploadCompleteRequest = z.infer<typeof UploadCompleteRequestSchema>;
export type Upload = z.infer<typeof UploadSchema>;
export type UploadPartsResponse = z.infer<typeof UploadPartsResponseSchema>;

export const SecretMetadataSchema = z.strictObject({
  name: RegisteredNameSchema,
  revision: positiveInteger,
  state: z.enum(["ready", "revoked"]),
  createdAt: timestamp,
  updatedAt: timestamp,
  revokedAt: z.optional(timestamp)
});
export const SecretSetRequestSchema = z.strictObject({
  value: nonEmptyString
});
export const SecretRevocationSchema = z.strictObject({
  name: RegisteredNameSchema,
  revision: positiveInteger,
  revokedAt: timestamp
});
export type SecretMetadataV1 = z.infer<typeof SecretMetadataSchema>;
export type SecretRevocation = z.infer<typeof SecretRevocationSchema>;

const approvalBase = {
  id: resourceId("approval"),
  sessionId: resourceId("session"),
  runId: resourceId("run"),
  agentId: resourceId("agent"),
  toolCallId: resourceId("toolCall"),
  toolName: nonEmptyString,
  argumentsSha256: sha256,
  implementationSha256: sha256,
  expectedGenerationId: resourceId("generation"),
  requestedAt: timestamp,
  updatedAt: timestamp
} as const;

export const ApprovalSchema = z.discriminatedUnion("status", [
  z.strictObject({ ...approvalBase, status: z.literal("pending") }),
  z.strictObject({
    ...approvalBase,
    status: z.literal("approved"),
    decision: z.literal("approve"),
    resolvedAt: timestamp
  }),
  z.strictObject({
    ...approvalBase,
    status: z.literal("denied"),
    decision: z.literal("deny"),
    resolvedAt: timestamp
  }),
  z.strictObject({
    ...approvalBase,
    status: z.literal("cancelled"),
    resolvedAt: timestamp
  })
]);
export const ApprovalResponseRequestSchema = z.strictObject({
  decision: z.enum(["approve", "deny"])
});
export type Approval = z.infer<typeof ApprovalSchema>;
export type ApprovalResponseRequest = z.infer<typeof ApprovalResponseRequestSchema>;
