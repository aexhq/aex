import { describe, expect, it } from "bun:test";
import * as contractRoot from "../src/index.js";
import {
  ApiErrorSchema,
  BOOTSTRAP_API_ROUTE_DESCRIPTORS,
  EffectiveWorkspaceLimitSchema,
  MessageSchema,
  OperationSchema,
  PageSchema,
  REGIONAL_API_ROUTE_DESCRIPTORS,
  RunSchema,
  SessionCreateRequestSchema,
  SessionSchema,
  WorkspaceApiKeyValueSchema,
  newId
} from "../src/index.js";

const at = "2026-07-30T12:00:00.000Z";
const hash = `sha256:${"a".repeat(64)}`;

const resolvedConfig = {
  builtinCatalogHash: hash,
  builtinTools: ["bash", "read_file"],
  approvalPolicy: { mode: "allow_all" as const },
  network: { hands: { mode: "none" as const } },
  packages: [],
  compute: {
    size: "1gb" as const,
    baseline: { memoryMiB: 1024, vcpus: 0.5 },
    peak: { memoryMiB: 4096, vcpus: 2 },
    maxDiskGiB: 8,
    endpointBandwidthMBps: 2,
    maxConcurrentConnections: 16
  },
  continuityPolicy: {
    idleAction: "hibernate" as const,
    idleDelayMs: 180_000,
    warmRetention: "provider_lifetime" as const,
    hardLifetimeMs: 28_800_000
  },
  harness: {
    protocol: "aex-agent-v1" as const,
    revision: hash,
    platformPrompt: "required" as const,
    instructionDiscovery: "none" as const,
    toolResultContextLimitBytes: 65_536
  }
};

const session = {
  id: newId("session"),
  workspaceId: newId("workspace"),
  status: "idle" as const,
  revision: 1,
  persistRevision: 0,
  createdAt: at,
  updatedAt: at,
  continuity: {
    state: "cold" as const,
    persistedRevision: 0,
    changedAt: at,
    reason: "not_started" as const
  },
  lineage: {},
  resolvedConfig
};

function parses(
  schema: { readonly "~standard": { validate(value: unknown): unknown } },
  value: unknown
): boolean {
  const result = schema["~standard"].validate(value) as
    | { readonly issues: readonly unknown[] | undefined }
    | Promise<unknown>;
  if (result instanceof Promise) throw new Error("v1 schemas must validate synchronously");
  return result.issues === undefined;
}

describe("v1 core response schemas", () => {
  it("pins the nested ApiError envelope and generic page", () => {
    expect(parses(ApiErrorSchema, {
      error: { code: "invalid_request", message: "bad", requestId: "req_1", retryable: false }
    })).toBe(true);
    expect(parses(ApiErrorSchema, { error: "invalid_request", message: "bad" })).toBe(false);

    const schema = PageSchema(SessionSchema);
    expect(parses(schema, { items: [session], nextCursor: "cur_example" })).toBe(true);
    expect(parses(schema, { sessions: [session] })).toBe(false);
  });

  it("accepts exact Session, Message, and Run resources and rejects retired fields", () => {
    expect(parses(SessionSchema, session)).toBe(true);
    for (const retired of [
      { runtime: { kind: "lambda" } },
      { checkpointId: "cp_1" },
      { suspendedAt: at },
      { continuityPolicy: resolvedConfig.continuityPolicy }
    ]) {
      expect(parses(SessionSchema, { ...session, ...retired })).toBe(false);
    }

    const message = {
      id: newId("message"),
      sessionId: session.id,
      runId: newId("run"),
      role: "user" as const,
      content: [{ type: "text" as const, text: "hello" }],
      createdAt: at
    };
    const run = {
      id: message.runId,
      sessionId: session.id,
      messageId: message.id,
      status: "queued" as const,
      maxSpendCents: 100,
      queuedAt: at
    };
    expect(parses(MessageSchema, message)).toBe(true);
    expect(parses(RunSchema, run)).toBe(true);
    expect(parses(RunSchema, { ...run, status: "finalizing" })).toBe(false);
  });
});

describe("workspace API-key secret values", () => {
  it("pins region, embedded key identity, and 32 random bytes as canonical unpadded base64url", () => {
    const keyId = newId("apiKey");
    const suffix = keyId.slice("key_".length);
    for (const region of ["use1", "use2", "usw2", "apne1", "euw1"]) {
      expect(parses(
        WorkspaceApiKeyValueSchema,
        `aex_wk_${region}_${suffix}_${"A".repeat(43)}`
      )).toBe(true);
    }
  });

  it.each([
    "aex_dev_euw1_wsp1_secret_crc",
    `aex_wk_euw1_${newId("apiKey").slice(4)}_${"A".repeat(42)}`,
    `aex_wk_euw1_${newId("apiKey").slice(4)}_${"A".repeat(44)}`,
    `aex_wk_euw1_${newId("apiKey").slice(4)}_${"A".repeat(42)}B`,
    `aex_wk_euw1_${newId("apiKey").slice(4)}_${"A".repeat(43)}_crc`
  ])("rejects noncanonical or legacy key %s", (value) => {
    expect(parses(WorkspaceApiKeyValueSchema, value)).toBe(false);
  });
});

describe("v1 session creation", () => {
  it("requires model and accepts only the explicit v1 inputs", () => {
    expect(parses(SessionCreateRequestSchema, { model: "anthropic/claude-sonnet" })).toBe(true);
    expect(parses(SessionCreateRequestSchema, {
      model: "anthropic/claude-sonnet",
      compute: { size: "4gb" }
    })).toBe(true);
    expect(parses(SessionCreateRequestSchema, {})).toBe(false);
    expect(parses(SessionCreateRequestSchema, { model: "" })).toBe(false);
    expect(parses(SessionCreateRequestSchema, {
      model: "m",
      compute: { requestedSize: "1gb", peakSize: "4gb", diskSize: "8gb" }
    })).toBe(false);
  });

  it.each([
    ["runtime", { kind: "lambda" }],
    ["runtimeKind", "lambda"],
    ["checkpoint", true],
    ["checkpointId", "cp_1"],
    ["suspend", true],
    ["resume", true],
    ["continuityPolicy", resolvedConfig.continuityPolicy],
    ["profile", "standard"],
    ["durable", true],
    ["ephemeral", false],
    ["region", "eu-west-1"]
  ])("rejects removed field %s", (field, value) => {
    expect(parses(SessionCreateRequestSchema, { model: "m", [field]: value })).toBe(false);
  });
});

describe("durable operation union", () => {
  const base = {
    id: newId("operation"),
    workspaceId: session.workspaceId,
    status: "succeeded" as const,
    cancelable: false,
    createdAt: at,
    updatedAt: at,
    terminalAt: at
  };

  const cases = [
    ["session_stop", { sessionId: session.id, changed: true, sessionRevision: 2 }],
    ["session_persist", {
      sessionId: session.id, changed: true, persistRevision: 1, rootHash: hash,
      lastPersistedAt: at, added: 1, updated: 0, deleted: 0, bytesMoved: 10
    }],
    ["session_fork", { session }],
    ["workspace_discard", { sessionId: session.id, changed: false, continuity: session.continuity }],
    ["session_delete", {
      sessionId: session.id, workspaceId: session.workspaceId, deletedAt: at,
      deletionOperationId: base.id, residualRetention: []
    }],
    ["credential_rebind", { sessionId: session.id, custodyRevision: 2, secrets: [{ name: "provider" }] }],
    ["telemetry_export", { exportId: newId("export"), format: "ndjson", manifestHash: hash, expiresAt: at }],
    ["workspace_delete", {
      workspaceId: session.workspaceId, deletedAt: at, deletionOperationId: base.id,
      residualRetention: []
    }]
  ] as const;

  it.each(cases)("binds %s to its exact result", (kind, result) => {
    expect(parses(OperationSchema, { ...base, kind, result })).toBe(true);
  });

  it("rejects a result borrowed from another operation kind", () => {
    expect(parses(OperationSchema, {
      ...base,
      kind: "session_stop",
      result: cases[6][1]
    })).toBe(false);
  });
});

describe("effective workspace limits", () => {
  it("returns only adjustable effective values with durable provenance", () => {
    expect(parses(EffectiveWorkspaceLimitSchema, {
      id: "query.page",
      effectiveValue: { maximumItems: 1_000, targetBytes: 8_388_608 },
      source: "workspace_override",
      adjustable: true,
      revision: 3,
      changedAt: at
    })).toBe(true);
    expect(parses(EffectiveWorkspaceLimitSchema, {
      id: "microvm.lifetime",
      effectiveValue: 28_800,
      source: "provider",
      adjustable: false,
      revision: 1,
      changedAt: at
    })).toBe(false);
  });
});

describe("exact v1 route authorities", () => {
  it("pins bootstrap routes, scopes, and idempotency header policy", () => {
    expect(BOOTSTRAP_API_ROUTE_DESCRIPTORS.map(({ name, method, samplePath, requiredScope, idempotency }) => [
      name, method, samplePath, requiredScope, idempotency
    ])).toEqual([
      ["account.get", "GET", "/account", "account:read", "none"],
      ["organizations.list", "GET", "/organizations", "organizations:read", "none"],
      ["organizations.create", "POST", "/organizations", "organizations:write", "idempotency-key"],
      ["organizations.get", "GET", "/organizations/org_1", "organizations:read", "none"],
      ["memberships.list", "GET", "/organizations/org_1/memberships", "memberships:read", "none"],
      ["invitations.create", "POST", "/organizations/org_1/invitations", "memberships:write", "idempotency-key"],
      ["workspaces.list", "GET", "/workspaces", "workspaces:read", "none"],
      ["workspaces.create", "POST", "/workspaces", "workspaces:write", "idempotency-key"],
      ["workspaces.get", "GET", "/workspaces/wsp_1", "workspaces:read", "none"],
      ["workspaces.delete", "POST", "/workspaces/wsp_1/deletions", "workspaces:delete", "operation-id"],
      ["apiKeys.list", "GET", "/api-keys", "api_keys:read", "none"],
      ["apiKeys.create", "POST", "/api-keys", "api_keys:write", "idempotency-key"],
      ["apiKeys.delete", "DELETE", "/api-keys/key_1", "api_keys:write", "none"],
      ["billing.balance", "GET", "/billing/balance", "billing:read", "none"],
      ["billing.topUpCheckout", "POST", "/organizations/org_1/billing/top-up-checkouts", "billing:write", "idempotency-key"],
      ["billing.portalSession", "POST", "/organizations/org_1/billing/portal-sessions", "billing:write", "idempotency-key"],
      ["billing.autoTopup.get", "GET", "/organizations/org_1/billing/auto-topup-policy", "billing:read", "none"],
      ["billing.autoTopup.put", "PUT", "/organizations/org_1/billing/auto-topup-policy", "billing:write", "idempotency-key"],
      ["billing.statements.list", "GET", "/organizations/org_1/billing/statements", "billing:read", "none"],
      ["billing.statements.get", "GET", "/organizations/org_1/billing/statements/stm_1", "billing:read", "none"],
      ["billing.statements.download", "POST", "/organizations/org_1/billing/statements/stm_1/downloads", "billing:read", "idempotency-key"],
      ["operations.list", "GET", "/operations", "operations:read", "none"],
      ["operations.get", "GET", "/operations/op_1", null, "none"],
      ["operations.cancel", "POST", "/operations/op_1/cancellations", "operations:write", "none"]
    ]);
  });

  it("pins regional session/run/operation routes and operation headers", () => {
    const foundationNames = new Set([
      "workspace.get", "workspace.limits.list", "workspace.limits.get",
      "sessions.create", "sessions.list", "sessions.get",
      "messages.list", "messages.send", "runs.list", "runs.get",
      "sessions.stop", "sessions.persist", "sessions.fork",
      "sessions.workspace.discard", "sessions.credentials.rebind",
      "sessions.delete", "operations.list", "operations.get", "operations.cancel"
    ]);
    expect(REGIONAL_API_ROUTE_DESCRIPTORS
      .filter(({ name }) => foundationNames.has(name))
      .map(({ name, method, samplePath, requiredScope, idempotency }) => [
      name, method, samplePath, requiredScope, idempotency
      ])).toEqual([
      ["workspace.get", "GET", "/workspace", "workspace:read", "none"],
      ["workspace.limits.list", "GET", "/workspace/limits", "workspace:read", "none"],
      ["workspace.limits.get", "GET", "/workspace/limits/query.page", "workspace:read", "none"],
      ["sessions.create", "POST", "/sessions", "sessions:write", "idempotency-key"],
      ["sessions.list", "GET", "/sessions", "sessions:read", "none"],
      ["sessions.get", "GET", "/sessions/ses_1", "sessions:read", "none"],
      ["messages.list", "GET", "/sessions/ses_1/messages", "sessions:read", "none"],
      ["messages.send", "POST", "/sessions/ses_1/messages", "sessions:write", "idempotency-key"],
      ["runs.list", "GET", "/sessions/ses_1/runs", "sessions:read", "none"],
      ["runs.get", "GET", "/sessions/ses_1/runs/run_1", "sessions:read", "none"],
      ["sessions.stop", "POST", "/sessions/ses_1/stops", "sessions:write", "operation-id"],
      ["sessions.persist", "POST", "/sessions/ses_1/persists", "files:write", "operation-id"],
      ["sessions.fork", "POST", "/sessions/ses_1/forks", "sessions:write", "operation-id"],
      ["sessions.workspace.discard", "POST", "/sessions/ses_1/workspace/discards", "sessions:write", "operation-id"],
      ["sessions.credentials.rebind", "POST", "/sessions/ses_1/credential-rebinds", "secrets:write", "operation-id"],
      ["sessions.delete", "POST", "/sessions/ses_1/deletions", "sessions:delete", "operation-id"],
      ["operations.list", "GET", "/operations", "operations:read", "none"],
      ["operations.get", "GET", "/operations/op_1", "operations:read", "none"],
      ["operations.cancel", "POST", "/operations/op_1/cancellations", "operations:write", "none"]
    ]);
  });

  it("has no legacy runtime, checkpoint, child-session, archive, or webhook route", () => {
    const text = [...BOOTSTRAP_API_ROUTE_DESCRIPTORS, ...REGIONAL_API_ROUTE_DESCRIPTORS]
      .map((route) => `${route.name} ${route.samplePath}`)
      .join("\n");
    for (const retired of [
      "suspend", "resume", "checkpoint", "runtime", "children", "child",
      "webhook", "archive", "events/ticket", "request-approval"
    ]) {
      expect(text).not.toContain(retired);
    }
  });
});

describe("clean v1 contract root", () => {
  it("does not export removed submission, runtime, archive, or compatibility owners", () => {
    for (const removed of [
      "parseSubmission",
      "RuntimeKindSchema",
      "RuntimeSizeSchema",
      "SessionArchiveSchema",
      "AssetRefSchema",
      "WorkspaceResourceVersionSchema",
      "ConnectionTicketSchema",
      "streamCoordinatorEvents",
      "ContentDeletedError",
      "SessionConfigValidationError"
    ]) {
      expect(contractRoot).not.toHaveProperty(removed);
    }
    for (const removed of [
      "checkpoint_not_available",
      "event_archive_too_large",
      "event_archive_deadline_exceeded",
      "insufficient_credits",
      "account_blocked",
      "content_deleted"
    ]) {
      expect(contractRoot.AEX_API_ERROR_CODES).not.toContain(removed);
    }
  });
});
