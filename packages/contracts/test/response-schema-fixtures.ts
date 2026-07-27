/**
 * The shared harness and the realistic response bodies the P3 table in
 * `response-schemas.test.ts` accepts and mutates into a plausible violation.
 *
 * Extracted from that suite so the fixtures can be read on their own: a fixture
 * is a claim about what the server actually sends, and a reader checking one
 * against a route handler should not have to scroll a 100-entry case table to
 * find it. The suite still owns every assertion — nothing here calls `expect`.
 */
import type { StandardSchemaV1 } from "@standard-schema/spec";
import { runtimeProfilesFixture } from "./runtime-profile-fixture.js";

export const TS = "2026-07-25T12:00:00.000Z";

export function validate(schema: StandardSchemaV1, value: unknown): StandardSchemaV1.Result<unknown> {
  const result = schema["~standard"].validate(value);
  if (result instanceof Promise) {
    throw new Error("response schemas must validate synchronously — the harness cannot await");
  }
  return result;
}

// ===========================================================================
// Fixtures
// ===========================================================================

export const checkpoint = {
  checkpointId: "cp_01",
  runId: "sess_1:turn:1",
  turnSeq: 1,
  committedAt: TS,
  throughSeq: 42
};

/** A session exactly as `publicSessionFromItem` emits it: FLAT runtime fields. */
export const sessionWire = {
  id: "sess_1",
  status: "idle",
  acceptsMessages: true,
  lastRun: {
    sessionId: "sess_1",
    turnSeq: 1,
    runId: "sess_1:turn:1",
    phase: "finished",
    outcome: "succeeded",
    startedAt: TS,
    finishedAt: TS,
    checkpoint
  },
  runtimeSize: "0.25cpu-1gb",
  runtimeKind: "lambda",
  workspaceId: "wsp_abc",
  createdAt: TS,
  updatedAt: TS,
  activeDurationMs: 4200,
  provider: "anthropic",
  model: "anthropic/claude-sonnet-4",
  retainedStorageBytes: 1024,
  costUsd: 0.0123,
  costTelemetry: {
    schemaVersion: 1,
    billedCostUsd: 0.0123,
    costBasis: { currency: "USD", status: "estimated" },
    basis: "session_turn",
    durationMs: 4200,
    turnSeq: 1,
    provider: "anthropic",
    model: "anthropic/claude-sonnet-4",
    providerUsage: [{ provider: "anthropic", inputTokens: 10, outputTokens: 20, totalTokens: 30 }],
    files: { capturedBytes: 1024, capturedFiles: 1 },
    storage: { storedBytes: 1024, storedFiles: 1, byteMilliseconds: 0 }
  },
  dataState: "active"
};

export const sessionFile = {
  id: "file_1",
  checkpointId: "cp_01",
  filename: "out/report.md",
  sizeBytes: 128,
  contentType: "text/markdown",
  createdAt: TS,
  sha256: "a".repeat(64)
};

/**
 * A real `GET /billing` body. Quotas and units come from the server, so the
 * fixture states plausible ones rather than importing the hosted table: the
 * schema checks the SHAPE, and pinning the policy here would be the second copy
 * the prepaid model exists to remove.
 */
export const billingSummary = {
  balanceUsd: 5,
  monthSpendUsd: 1.25,
  spendCapUsd: 0,
  period: "2026-07",
  admissionState: "carded_manual",
  accountType: "standard",
  paymentMethodStatus: "active",
  autoTopupEnabled: false,
  blocked: null,
  paymentMethod: { present: true, brand: "visa", last4: "4242" },
  autoTopup: {
    enabled: false,
    thresholdUsd: 5,
    amountUsd: 20,
    minimumAmountUsd: 10,
    maxPerDay: 4
  },
  allowances: [
    {
      dimension: "llm_token_usd",
      quota: 2,
      used: 0.5,
      remaining: 1.5,
      unit: "USD",
      label: "model usage",
      resetAt: TS,
      approximateTokens: { model: "anthropic/claude-haiku-4-5", tokens: 1_200_000 }
    },
    {
      dimension: "egress_gb",
      quota: 5,
      used: 1.4,
      remaining: 3.6,
      unit: "GB",
      label: "egress",
      resetAt: TS
    }
  ]
};

export const whoami = {
  ok: true,
  principalType: "api_key",
  workspaceId: "wsp_abc",
  scopes: ["sessions:read", "sessions:write"],
  limits: {
    maxConcurrentSessions: 10,
    submitRatePerMinute: 60,
    spendCapUsd: 0,
    monthSpendUsd: 1.25,
    balanceUsd: 5,
    balanceGraceFloorUsd: 0,
    llmTokenAllowanceRemainingUsd: 0,
    creditGateActive: true,
    paymentMethodStatus: "none",
    admissionState: "free",
    autoTopupEnabled: false,
    accountType: "standard"
  },
  runtimeCapabilities: {
    schemaVersion: 1,
    capabilityVersion: "runtime-capabilities.v1",
    capabilityHash: `sha256:${"b".repeat(64)}`,
    availableRuntimeKinds: ["lambda"],
    sizesByRuntimeKind: { lambda: ["0.25cpu-1gb", "1cpu-6gb"] },
    unavailable: {
      container: { code: "runtime_unavailable" },
      spot_container: { code: "runtime_unavailable" }
    },
    profilesByRuntimeKind: runtimeProfilesFixture()
  }
};

export const workspaceResourceCommon = {
  resourceId: "wres_1",
  name: "notes",
  version: 1,
  assetId: `asset_${"c".repeat(64)}`,
  contentHash: `sha256:${"c".repeat(64)}`,
  sizeBytes: 12,
  contentType: "text/markdown",
  createdAt: TS
};

export const secret = {
  id: "sec_1",
  name: "OPENAI_KEY",
  version: 2,
  state: "ready",
  createdAt: TS,
  updatedAt: TS
};

export const mcpServer = {
  id: "mcp_abcdefghi",
  workspaceId: "wsp_abc",
  name: "linear",
  url: "https://mcp.linear.app/sse",
  headerShape: ["authorization"],
  createdAt: TS,
  updatedAt: TS
};

export const delivery = {
  id: "whd_sess_1:turn:1",
  runId: "sess_1:turn:1",
  turnSeq: 1,
  eventType: "run.finished",
  status: "delivered",
  attemptCount: 1,
  createdAt: TS
};

export interface Case {
  readonly name: string;
  readonly schema: StandardSchemaV1;
  readonly accepts: unknown;
  readonly rejects: unknown;
  /** What the rejection must say — proof it failed for the intended reason. */
  readonly because: string;
}
