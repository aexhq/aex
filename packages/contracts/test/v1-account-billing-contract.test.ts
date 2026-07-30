import { describe, expect, it } from "bun:test";
import {
  AEX_API_ERROR_CODES,
  AccountOperationalStateSchema,
  ApiKeySchema,
  AutoTopupPolicyRequestSchema,
  AutoTopupPolicySchema,
  BOOTSTRAP_API_ROUTE_DESCRIPTORS,
  BillingBalanceSchema,
  InvitationSchema,
  NewApiKeySchema,
  OrganizationSchema,
  REGIONAL_API_ROUTE_DESCRIPTORS,
  StatementSchema,
  UsagePageSchema,
  UsageQuerySchema,
  WorkspaceCreateRequestSchema,
  WorkspaceSchema,
  newId
} from "../src/index.js";

const at = "2026-07-30T10:00:00.000Z";
const later = "2026-07-31T10:00:00.000Z";
const hash = `sha256:${"a".repeat(64)}`;

function accepts(
  schema: { safeParse(value: unknown): { success: boolean } },
  value: unknown
): boolean {
  return schema.safeParse(value).success;
}

describe("v1 account, organization, workspace, and key contracts", () => {
  it("pins active|paused account state and inherited workspace state", () => {
    const organizationId = newId("organization");
    expect(accepts(AccountOperationalStateSchema, {
      status: "active",
      revision: 2,
      changedAt: at
    })).toBe(true);
    expect(accepts(AccountOperationalStateSchema, {
      status: "paused",
      reason: "top_up_required",
      revision: 3,
      changedAt: at,
      minimumRestoreCents: "1250",
      retentionFundedUntil: later,
      deletionScheduledAt: later
    })).toBe(true);
    expect(accepts(WorkspaceSchema, {
      id: newId("workspace"),
      organizationId,
      name: "Production",
      slug: "production",
      region: "eu-west-1",
      apiUrl: "https://eu-west-1.api.aex.dev",
      status: "active",
      operationalState: {
        status: "active",
        revision: 2,
        changedAt: at,
        inheritedFrom: "account",
        organizationId
      },
      createdAt: at
    })).toBe(true);
    expect(accepts(WorkspaceSchema, {
      id: newId("workspace"),
      organizationId,
      name: "Invalid inheritance",
      slug: "invalid-inheritance",
      region: "eu-west-1",
      apiUrl: "https://eu-west-1.api.aex.dev",
      status: "active",
      operationalState: {
        status: "active",
        revision: 2,
        changedAt: at,
        inheritedFrom: "account",
        organizationId: newId("organization")
      },
      createdAt: at
    })).toBe(false);
  });

  it("requires explicit immutable placement and separates key creation", () => {
    expect(accepts(WorkspaceCreateRequestSchema, {
      organizationId: newId("organization"),
      name: "Production",
      region: "eu-west-1"
    })).toBe(true);
    expect(accepts(WorkspaceCreateRequestSchema, {
      organizationId: newId("organization"),
      name: "Implicit"
    })).toBe(false);
    expect(accepts(WorkspaceCreateRequestSchema, {
      organizationId: newId("organization"),
      name: "Combined",
      region: "eu-west-1",
      apiKey: true
    })).toBe(false);
  });

  it("pins organization/invitation and one-time workspace-key records", () => {
    const organizationId = newId("organization");
    const workspaceId = newId("workspace");
    expect(accepts(OrganizationSchema, {
      id: organizationId,
      name: "Example",
      slug: "example",
      callerRole: "owner",
      createdAt: at
    })).toBe(true);
    expect(accepts(InvitationSchema, {
      id: newId("invitation"),
      organizationId,
      email: "member@example.com",
      role: "member",
      status: "pending",
      createdAt: at,
      expiresAt: later
    })).toBe(true);
    const key = {
      id: newId("apiKey"),
      workspaceId,
      name: "automation",
      scopes: ["sessions:write"],
      createdAt: at,
      revokedAt: null
    };
    expect(accepts(ApiKeySchema, key)).toBe(true);
    expect(accepts(ApiKeySchema, { ...key, lastUsedAt: at })).toBe(false);
    expect(accepts(NewApiKeySchema, {
      ...key,
      value: `aex_wk_euw1_${key.id.slice(4)}_${"A".repeat(43)}`
    })).toBe(true);
  });
});

describe("v1 billing, statements, and regional usage", () => {
  it("keeps auto-topup USD field names with exact whole-cent replacement", () => {
    expect(accepts(AutoTopupPolicyRequestSchema, {
      enabled: false,
      thresholdUsd: 5,
      amountUsd: 20
    })).toBe(true);
    expect(accepts(AutoTopupPolicySchema, {
      enabled: false,
      thresholdUsd: 5,
      amountUsd: 20,
      revision: 1,
      updatedAt: at
    })).toBe(true);
    for (const request of [
      { enabled: true, thresholdUsd: 0, amountUsd: 20 },
      { enabled: true, thresholdUsd: 19.999, amountUsd: 20 },
      { enabled: true, thresholdUsd: 20, amountUsd: 20 },
      { enabled: true, thresholdUsd: 5, amountUsd: 9.99 },
      { enabled: true, thresholdUsd: 5, amountUsd: 500.01 },
      { enabled: true, thresholdUsd: 5 }
    ]) {
      expect(accepts(AutoTopupPolicyRequestSchema, request)).toBe(false);
    }
  });

  it("uses integer-cent strings for balances and immutable statements", () => {
    const organizationId = newId("organization");
    const operationalState = { status: "active", revision: 4, changedAt: at };
    expect(accepts(BillingBalanceSchema, {
      organizationId,
      currency: "USD",
      revision: 4,
      availableCents: "1200",
      reservedCents: "200",
      pendingCents: "0",
      operationalState,
      updatedAt: at
    })).toBe(true);
    expect(accepts(StatementSchema, {
      id: newId("statement"),
      organizationId,
      period: { gte: at, lt: later },
      currency: "USD",
      totalCents: "325",
      lines: [
        { category: "storage", totalCents: "25" },
        { category: "compute", totalCents: "300" }
      ],
      artifactHash: hash,
      issuedAt: later
    })).toBe(true);
    expect(accepts(BillingBalanceSchema, {
      organizationId,
      currency: "USD",
      revision: 4,
      availableCents: 1200,
      reservedCents: "200",
      pendingCents: "0",
      operationalState,
      updatedAt: at
    })).toBe(false);
  });

  it("pins typed usage rows and category/region/workspace frontier vectors", () => {
    const workspaceId = newId("workspace");
    const query = {
      categories: ["compute"],
      timeRange: { gte: at, lt: later },
      groupBy: ["category", "workspace"],
      limit: 100
    };
    expect(accepts(UsageQuerySchema, query)).toBe(true);
    expect(accepts(UsagePageSchema, {
      items: [{
        category: "compute",
        region: "eu-west-1",
        workspaceId,
        source: "microvm",
        ratedCents: "23",
        serviceTime: { gte: at, lt: later },
        quantity: {
          kind: "resource_time",
          durationMilliseconds: "1000",
          allocatedMemoryBytes: "1073741824",
          allocatedVcpuMillis: "500"
        }
      }],
      frontiers: [{
        region: "eu-west-1",
        workspaceId,
        category: "compute",
        acceptedSequence: "10",
        ratedSequence: "9",
        aggregatedSequence: "8",
        settledSequence: "7",
        serviceThrough: later
      }]
    })).toBe(true);
  });
});

describe("v1 control/billing route and error authority", () => {
  it("pins operation, idempotency, download-grant, and regional-query routes", () => {
    const selected = new Set([
      "workspaces.delete",
      "billing.autoTopup.put",
      "billing.statements.download"
    ]);
    expect(BOOTSTRAP_API_ROUTE_DESCRIPTORS
      .filter(({ name }) => selected.has(name))
      .map(({ name, method, idempotency }) => [name, method, idempotency]))
      .toEqual([
        ["workspaces.delete", "POST", "operation-id"],
        ["billing.autoTopup.put", "PUT", "idempotency-key"],
        ["billing.statements.download", "POST", "idempotency-key"]
      ]);
    expect(REGIONAL_API_ROUTE_DESCRIPTORS
      .filter(({ name }) => name === "billing.usage.query")
      .map(({ method, requiredScope, idempotency }) => [
        method, requiredScope, idempotency
      ])).toEqual([["POST", "billing:read", "none"]]);
  });

  it("declares pause/account errors and no legacy billing/control routes", () => {
    for (const code of [
      "invalid_auto_topup_policy",
      "payment_method_required",
      "authentication_unavailable",
      "account_paused",
      "account_state_unavailable",
      "precondition_failed",
      "wrong_workspace_region"
    ] as const) {
      expect(AEX_API_ERROR_CODES).toContain(code);
    }
    const routes = [
      ...BOOTSTRAP_API_ROUTE_DESCRIPTORS,
      ...REGIONAL_API_ROUTE_DESCRIPTORS
    ].map(({ samplePath }) => samplePath);
    for (const removed of [
      "/plans",
      "/subscriptions",
      "/allowances",
      "/billing/topup/checkout",
      "/billing/portal",
      "/personal-access-tokens"
    ]) {
      expect(routes).not.toContain(removed);
    }
  });
});
