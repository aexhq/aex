/** Strict prelaunch v1 bootstrap account/workspace and regional usage contracts. */
import * as z from "zod/mini";
import { isId, type Id, type IdKind } from "./ids.js";
import { RegionSchema, WorkspaceApiKeyValueSchema, PageSchema } from "./v1-resources.js";

const timestamp = z.string().check(
  z.regex(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/)
);
const nonEmptyString = z.string().check(z.minLength(1));
const positiveInteger = z.int().check(z.gte(1));
const nonNegativeDecimalInteger = z.string().check(z.regex(/^(0|[1-9]\d*)$/));
const sha256 = z.string().check(z.regex(/^sha256:[0-9a-f]{64}$/));
const slug = z.string().check(z.regex(/^[a-z0-9]+(?:-[a-z0-9]+)*$/));
const email = z.string().check(z.regex(/^[^@\s]+@[^@\s]+\.[^@\s]+$/));
const httpsUrl = z.string().check(z.regex(/^https:\/\/[^/\s]+(?:\/.*)?$/));

function resourceId<K extends IdKind>(kind: K) {
  return z.string().check(z.refine((value): value is Id<K> => isId(kind, value)));
}

export const AccountOperationalStateSchema = z.discriminatedUnion("status", [
  z.strictObject({
    status: z.literal("active"),
    revision: positiveInteger,
    changedAt: timestamp
  }),
  z.strictObject({
    status: z.literal("paused"),
    reason: z.literal("top_up_required"),
    revision: positiveInteger,
    changedAt: timestamp,
    minimumRestoreCents: z.optional(nonNegativeDecimalInteger),
    retentionFundedUntil: z.optional(timestamp),
    deletionScheduledAt: z.optional(timestamp)
  })
]);
export type AccountOperationalState =
  z.infer<typeof AccountOperationalStateSchema>;

const inheritedWorkspaceState = {
  inheritedFrom: z.literal("account"),
  organizationId: resourceId("organization")
} as const;

export const WorkspaceOperationalStateSchema = z.discriminatedUnion("status", [
  z.strictObject({
    status: z.literal("active"),
    revision: positiveInteger,
    changedAt: timestamp,
    ...inheritedWorkspaceState
  }),
  z.strictObject({
    status: z.literal("paused"),
    reason: z.literal("top_up_required"),
    revision: positiveInteger,
    changedAt: timestamp,
    minimumRestoreCents: z.optional(nonNegativeDecimalInteger),
    retentionFundedUntil: z.optional(timestamp),
    deletionScheduledAt: z.optional(timestamp),
    ...inheritedWorkspaceState
  })
]);
export type WorkspaceOperationalState =
  z.infer<typeof WorkspaceOperationalStateSchema>;

export const OrganizationSchema = z.strictObject({
  id: resourceId("organization"),
  name: nonEmptyString,
  slug,
  callerRole: z.enum(["owner", "admin", "member"]),
  createdAt: timestamp
});
export type Organization = z.infer<typeof OrganizationSchema>;
export const OrganizationPageSchema = PageSchema(OrganizationSchema);

export const OrganizationCreateRequestSchema = z.strictObject({
  name: nonEmptyString
});
export type OrganizationCreateRequest =
  z.infer<typeof OrganizationCreateRequestSchema>;

export const MembershipSchema = z.strictObject({
  id: resourceId("membership"),
  organizationId: resourceId("organization"),
  userId: resourceId("user"),
  email,
  role: z.enum(["owner", "admin", "member"]),
  status: z.literal("active"),
  createdAt: timestamp
});
export type Membership = z.infer<typeof MembershipSchema>;
export const MembershipPageSchema = PageSchema(MembershipSchema);

export const InvitationSchema = z.strictObject({
  id: resourceId("invitation"),
  organizationId: resourceId("organization"),
  email,
  role: z.enum(["admin", "member"]),
  status: z.enum(["pending", "accepted", "revoked", "expired"]),
  createdAt: timestamp,
  expiresAt: timestamp,
  resolvedAt: z.optional(timestamp)
});
export type Invitation = z.infer<typeof InvitationSchema>;

export const InvitationCreateRequestSchema = z.strictObject({
  email,
  role: z.enum(["admin", "member"])
});
export type InvitationCreateRequest =
  z.infer<typeof InvitationCreateRequestSchema>;

export const WorkspaceSchema = z.strictObject({
  id: resourceId("workspace"),
  organizationId: resourceId("organization"),
  name: nonEmptyString,
  slug,
  region: RegionSchema,
  apiUrl: httpsUrl,
  status: z.enum(["active", "deleting"]),
  operationalState: WorkspaceOperationalStateSchema,
  createdAt: timestamp,
  deletionOperationId: z.optional(resourceId("operation"))
}).check(z.refine((workspace) =>
  workspace.operationalState.organizationId === workspace.organizationId
));
export type Workspace = z.infer<typeof WorkspaceSchema>;
export const WorkspacePageSchema = PageSchema(WorkspaceSchema);

export const WorkspaceCreateRequestSchema = z.strictObject({
  organizationId: resourceId("organization"),
  name: nonEmptyString,
  region: RegionSchema
});
export type WorkspaceCreateRequest =
  z.infer<typeof WorkspaceCreateRequestSchema>;

export const WorkspaceDeleteRequestSchema = z.strictObject({
  confirmation: resourceId("workspace")
});
export type WorkspaceDeleteRequest =
  z.infer<typeof WorkspaceDeleteRequestSchema>;

const apiKeyBase = {
  id: resourceId("apiKey"),
  workspaceId: resourceId("workspace"),
  name: nonEmptyString,
  scopes: z.array(nonEmptyString),
  createdAt: timestamp,
  revokedAt: z.nullable(timestamp)
} as const;

export const ApiKeySchema = z.strictObject(apiKeyBase);
export type ApiKey = z.infer<typeof ApiKeySchema>;
export const ApiKeyPageSchema = PageSchema(ApiKeySchema);

export const ApiKeyCreateRequestSchema = z.strictObject({
  workspaceId: resourceId("workspace"),
  name: nonEmptyString,
  scopes: z.array(nonEmptyString)
});
export type ApiKeyCreateRequest = z.infer<typeof ApiKeyCreateRequestSchema>;

export const NewApiKeySchema = z.strictObject({
  ...apiKeyBase,
  value: WorkspaceApiKeyValueSchema
});
export type NewWorkspaceApiKey = z.infer<typeof NewApiKeySchema>;

function isWholeCentUsd(value: unknown): value is number {
  return typeof value === "number" &&
    Number.isFinite(value) &&
    Math.round(value * 100) / 100 === value;
}

const wholeCentUsd = z.number().check(z.refine(isWholeCentUsd));

function validAutoTopup(value: unknown): boolean {
  if (value === null || typeof value !== "object") return false;
  const policy = value as {
    readonly thresholdUsd?: unknown;
    readonly amountUsd?: unknown;
  };
  return isWholeCentUsd(policy.thresholdUsd) &&
    isWholeCentUsd(policy.amountUsd) &&
    policy.amountUsd >= 10 &&
    policy.amountUsd <= 500 &&
    policy.thresholdUsd >= 0.01 &&
    policy.thresholdUsd <= policy.amountUsd - 0.01;
}

const autoTopupPolicyFields = {
  enabled: z.boolean(),
  thresholdUsd: wholeCentUsd,
  amountUsd: wholeCentUsd
} as const;

export const DEFAULT_AUTO_TOPUP_POLICY = Object.freeze({
  enabled: false,
  thresholdUsd: 5,
  amountUsd: 20
});

export const AutoTopupPolicyRequestSchema = z.strictObject(
  autoTopupPolicyFields
).check(z.refine(validAutoTopup));
export type AutoTopupPolicyRequest =
  z.infer<typeof AutoTopupPolicyRequestSchema>;

export const AutoTopupPolicySchema = z.strictObject({
  ...autoTopupPolicyFields,
  revision: positiveInteger,
  updatedAt: timestamp
}).check(z.refine(validAutoTopup));
export type AutoTopupPolicy = z.infer<typeof AutoTopupPolicySchema>;

export const BillingBalanceSchema = z.strictObject({
  organizationId: resourceId("organization"),
  currency: z.literal("USD"),
  revision: positiveInteger,
  availableCents: nonNegativeDecimalInteger,
  reservedCents: nonNegativeDecimalInteger,
  pendingCents: nonNegativeDecimalInteger,
  operationalState: AccountOperationalStateSchema,
  updatedAt: timestamp
});
export type BillingBalance = z.infer<typeof BillingBalanceSchema>;

export const TopUpCheckoutRequestSchema = z.strictObject({
  amountUsd: z.number().check(
    z.refine((value) => isWholeCentUsd(value) && value > 0)
  ),
  successUrl: httpsUrl,
  cancelUrl: httpsUrl
});
export type TopUpCheckoutRequest =
  z.infer<typeof TopUpCheckoutRequestSchema>;

export const PortalSessionRequestSchema = z.strictObject({
  returnUrl: httpsUrl
});
export type PortalSessionRequest =
  z.infer<typeof PortalSessionRequestSchema>;

export const HostedBillingSessionSchema = z.strictObject({
  url: httpsUrl,
  expiresAt: timestamp
});
export type HostedBillingSession =
  z.infer<typeof HostedBillingSessionSchema>;

const statementBase = {
  id: resourceId("statement"),
  organizationId: resourceId("organization"),
  period: z.strictObject({ gte: timestamp, lt: timestamp }),
  currency: z.literal("USD"),
  totalCents: nonNegativeDecimalInteger,
  artifactHash: sha256,
  issuedAt: timestamp
} as const;

export const StatementSummarySchema = z.strictObject(statementBase);
export type StatementSummary = z.infer<typeof StatementSummarySchema>;
export const StatementSummaryPageSchema = PageSchema(StatementSummarySchema);

export const StatementLineSchema = z.strictObject({
  category: z.enum(["storage", "compute", "data_transfer"]),
  totalCents: nonNegativeDecimalInteger
});
export type StatementLine = z.infer<typeof StatementLineSchema>;

export const StatementSchema = z.strictObject({
  ...statementBase,
  lines: z.array(StatementLineSchema)
}).check(z.refine((statement) =>
  statement.lines.reduce(
    (total, line) => total + BigInt(line.totalCents),
    0n
  ) === BigInt(statement.totalCents)
));
export type Statement = z.infer<typeof StatementSchema>;

export const USAGE_CATEGORIES = [
  "storage",
  "compute",
  "data_transfer"
] as const;
export type UsageCategory = (typeof USAGE_CATEGORIES)[number];

export const USAGE_GROUPINGS = [
  "category",
  "region",
  "workspace",
  "session",
  "run",
  "operation"
] as const;
export type UsageGrouping = (typeof USAGE_GROUPINGS)[number];

function unique(values: readonly unknown[]): boolean {
  return new Set(values).size === values.length;
}

export const UsageQuerySchema = z.strictObject({
  categories: z.optional(
    z.array(z.enum(USAGE_CATEGORIES)).check(z.minLength(1), z.refine(unique))
  ),
  timeRange: z.strictObject({ gte: timestamp, lt: timestamp }),
  groupBy: z.optional(
    z.array(z.enum(USAGE_GROUPINGS)).check(z.maxLength(6), z.refine(unique))
  ),
  cursor: z.optional(z.string().check(z.regex(/^cur_/))),
  limit: z.optional(z.int().check(z.gte(1), z.lte(1_000)))
}).check(z.refine((value) =>
  Date.parse(value.timeRange.lt) > Date.parse(value.timeRange.gte)
));
export type UsageQuery = z.infer<typeof UsageQuerySchema>;
export type OrganizationUsageQuery = Omit<UsageQuery, "cursor">;

const serviceTime = z.strictObject({ gte: timestamp, lt: timestamp });
const usageBase = {
  region: RegionSchema,
  workspaceId: resourceId("workspace"),
  sessionId: z.optional(resourceId("session")),
  runId: z.optional(resourceId("run")),
  operationId: z.optional(resourceId("operation")),
  source: nonEmptyString,
  ratedCents: nonNegativeDecimalInteger,
  serviceTime
} as const;

const storageQuantity = z.strictObject({
  kind: z.literal("byte_milliseconds"),
  byteMilliseconds: nonNegativeDecimalInteger
});

const computeQuantity = z.discriminatedUnion("kind", [
  z.strictObject({
    kind: z.literal("resource_time"),
    durationMilliseconds: nonNegativeDecimalInteger,
    allocatedMemoryBytes: nonNegativeDecimalInteger,
    allocatedVcpuMillis: nonNegativeDecimalInteger
  }),
  z.strictObject({
    kind: z.literal("model_tokens"),
    modelPriceDimension: nonEmptyString,
    inputTokens: nonNegativeDecimalInteger,
    outputTokens: nonNegativeDecimalInteger,
    cacheReadTokens: nonNegativeDecimalInteger,
    cacheWriteTokens: nonNegativeDecimalInteger
  }),
  z.strictObject({
    kind: z.literal("invocations"),
    priceDimension: nonEmptyString,
    count: nonNegativeDecimalInteger
  })
]);

const transferQuantity = z.strictObject({
  kind: z.literal("bytes"),
  measuredBytes: nonNegativeDecimalInteger,
  authorizedBytes: nonNegativeDecimalInteger
});

export const UsageAggregateSchema = z.discriminatedUnion("category", [
  z.strictObject({
    category: z.literal("storage"),
    ...usageBase,
    quantity: storageQuantity
  }),
  z.strictObject({
    category: z.literal("compute"),
    ...usageBase,
    quantity: computeQuantity
  }),
  z.strictObject({
    category: z.literal("data_transfer"),
    ...usageBase,
    quantity: transferQuantity
  })
]);
export type UsageAggregate = z.infer<typeof UsageAggregateSchema>;

export const UsageFrontierSchema = z.strictObject({
  region: RegionSchema,
  workspaceId: resourceId("workspace"),
  category: z.enum(USAGE_CATEGORIES),
  acceptedSequence: nonNegativeDecimalInteger,
  ratedSequence: nonNegativeDecimalInteger,
  aggregatedSequence: nonNegativeDecimalInteger,
  settledSequence: nonNegativeDecimalInteger,
  serviceThrough: timestamp
}).check(z.refine((frontier) => {
  const accepted = BigInt(frontier.acceptedSequence);
  const rated = BigInt(frontier.ratedSequence);
  const aggregated = BigInt(frontier.aggregatedSequence);
  const settled = BigInt(frontier.settledSequence);
  return accepted >= rated && rated >= aggregated && aggregated >= settled;
}));
export type UsageFrontier = z.infer<typeof UsageFrontierSchema>;

export const UsagePageSchema = z.strictObject({
  items: z.array(UsageAggregateSchema),
  nextCursor: z.optional(z.string().check(z.regex(/^cur_/))),
  frontiers: z.array(UsageFrontierSchema)
});
export type UsagePage = z.infer<typeof UsagePageSchema>;

export interface OrganizationUsageResult {
  readonly items: readonly UsageAggregate[];
  readonly frontiers: readonly UsageFrontier[];
}
