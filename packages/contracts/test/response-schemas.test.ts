/**
 * P3 — every response schema accepts a realistic body and rejects a plausible
 * violation.
 *
 * "Plausible" is doing work here. A schema that only rejects `42` proves
 * nothing; each rejection below is a drift that could actually happen and that
 * someone would want to hear about:
 *
 *   - a removed field reappearing (`whoami.caps`),
 *   - a CLIENT-side shape leaking onto the wire (`session.runtime`, the grouped
 *     form `operations.ts` builds, or `link.expiresAt`, which the SDK
 *     synthesises),
 *   - a secret value appearing on a metadata-only read,
 *   - a union branch answered with a mixture of two branches,
 *   - a field the server currently sends unconditionally going missing.
 *
 * Validation goes through `~standard`, not through Zod's own API, because
 * `~standard` is what the harness calls and what the package publishes.
 */
import type { StandardSchemaV1 } from "@standard-schema/spec";
import { describe, expect, it } from "bun:test";
import * as z from "zod/mini";
import {
  AssetFinalizeResponseSchema,
  AssetMpuAbortResponseSchema,
  AssetMpuPresignPartsResponseSchema,
  AssetPresignResponseSchema
} from "../src/schemas/response-assets.js";
import {
  AdminBillingAccountTypeResponseSchema,
  AdminBillingPaymentMethodResponseSchema,
  AdminBillingTopupResponseSchema,
  BillingAutoTopupResponseSchema,
  BillingHostedSessionResponseSchema,
  BillingLedgerResponseSchema,
  BillingSummaryResponseSchema
} from "../src/schemas/response-billing.js";
import {
  NoContentResponseSchema,
  responseSchemaRegistry
} from "../src/schemas/response-common.js";
import {
  AccountWhoAmIResponseSchema,
  WhoAmIResponseSchema
} from "../src/schemas/response-identity.js";
import {
  McpServerListResponseSchema,
  McpServerResponseSchema
} from "../src/schemas/response-mcp-servers.js";
import {
  SecretListResponseSchema,
  SecretResponseSchema
} from "../src/schemas/response-secrets.js";
import {
  AcknowledgedResponseSchema,
  CoordinatorTicketResponseSchema,
  EventArchiveLinkResponseSchema,
  SessionChildrenResponseSchema,
  SessionDeleteResponseSchema,
  SessionEnvelopeResponseSchema,
  SessionEventsPageResponseSchema,
  SessionFileLinkResponseSchema,
  SessionFilesResponseSchema,
  SessionListResponseSchema,
  SessionMessageAcceptedResponseSchema,
  SessionMessagesPageResponseSchema,
  SessionWebhookDeliveriesResponseSchema
} from "../src/schemas/response-sessions.js";
import {
  ChildFinalizeResponseSchema,
  ChildResultResponseSchema,
  SessionOtlpResponseSchema
} from "../src/schemas/response-sessions-internal.js";
import {
  WebhookSigningSecretResponseSchema,
  WorkspaceWebhookDeliveriesResponseSchema
} from "../src/schemas/response-webhooks.js";
import {
  WorkspaceEraseResponseSchema,
  WorkspaceFilePageResponseSchema,
  WorkspaceFileResponseSchema,
  WorkspaceInstructionResponseSchema,
  WorkspaceSkillResponseSchema,
  WorkspaceToolResponseSchema
} from "../src/schemas/response-workspace.js";
import {
  TS,
  billingSummary,
  checkpoint,
  delivery,
  mcpServer,
  secret,
  sessionFile,
  sessionWire,
  validate,
  whoami,
  workspaceInstructionRecord,
  workspaceResourceCommon,
  type Case
} from "./response-schema-fixtures.js";

const cases: readonly Case[] = [
  {
    name: "sessions.get / create / state changes",
    schema: SessionEnvelopeResponseSchema,
    accepts: { session: sessionWire },
    // The grouped `runtime` object is built CLIENT-side by normalizeSessionRuntime.
    // It appearing on the wire would mean the server started sending the SDK's
    // own projection back.
    rejects: { session: { ...sessionWire, runtime: { kind: "lambda", size: "1cpu-6gb" } } },
    because: "runtime"
  },
  {
    name: "sessions.list",
    schema: SessionListResponseSchema,
    accepts: { sessions: [sessionWire], nextCursor: "eyJ2IjoxfQ" },
    rejects: { sessions: [sessionWire], nextCursor: null },
    because: "nextCursor"
  },
  {
    name: "sessions.sendMessage",
    schema: SessionMessageAcceptedResponseSchema,
    accepts: {
      session: sessionWire,
      run: { sessionId: "sess_1", turnSeq: 2, runId: "sess_1:turn:2", phase: "starting", eventCursor: 7 },
      eventCursor: 7
    },
    rejects: {
      session: sessionWire,
      run: { sessionId: "sess_1", turnSeq: 2, runId: "sess_1:turn:2", phase: "launching" },
      eventCursor: 7
    },
    because: "run.phase"
  },
  {
    name: "sessions.delete",
    schema: SessionDeleteResponseSchema,
    accepts: { session: sessionWire, purgedSessionFileObjects: 3, cleanupComplete: true },
    // `cleanupComplete` is unconditional on the wire; losing it is exactly the
    // silent narrowing C4 exists to catch.
    rejects: { session: sessionWire, purgedSessionFileObjects: 3 },
    because: "cleanupComplete"
  },
  {
    name: "sessions.listMessages",
    schema: SessionMessagesPageResponseSchema,
    accepts: {
      messages: [
        {
          id: "sess_1:1:user",
          sender: "user",
          text: "hello",
          timestamp: TS,
          sequence: 1,
          content: [{ type: "text", text: "hello" }],
          turnSeq: 1
        }
      ]
    },
    rejects: {
      messages: [{ id: "sess_1:1:user", sender: "user", text: "hello", timestamp: TS, sequence: 1 }]
    },
    because: "content"
  },
  {
    name: "sessions.listEvents",
    schema: SessionEventsPageResponseSchema,
    accepts: {
      events: [
        {
          specversion: "1.0",
          id: "sess_1:1024",
          source: "agent",
          type: "RUN_STARTED",
          subject: "sess_1",
          threadId: "sess_1",
          runId: "sess_1:turn:1",
          time: TS,
          sequence: 1024,
          data: { turnSeq: 1 }
        }
      ],
      nextCursor: "eyJ2IjoxfQ"
    },
    rejects: {
      events: [
        {
          specversion: "1.0",
          id: "sess_1:1024",
          source: "agent",
          type: "RUN_STARTED",
          subject: "sess_1",
          threadId: "sess_1",
          runId: "sess_1:turn:1",
          time: TS,
          data: {}
        }
      ]
    },
    because: "sequence"
  },
  {
    name: "sessions.eventsTicket",
    schema: CoordinatorTicketResponseSchema,
    accepts: {
      ok: true,
      wsUrl: "wss://events.aex.dev/api/sessions/sess_1/subscribe",
      ticket: "abc.def",
      expiresAtMs: 1_800_000_000_000,
      region: "eu-west-1"
    },
    // `CoordinatorTicket` used to stop at these three keys while the server sent
    // five; it now declares all five. Dropping `region` would be a real change.
    rejects: { wsUrl: "wss://x/y", ticket: "abc.def", expiresAtMs: 1_800_000_000_000 },
    because: "ok"
  },
  {
    name: "sessions.listChildren",
    schema: SessionChildrenResponseSchema,
    accepts: {
      children: [
        {
          id: "sess_child",
          parentSessionId: "sess_1",
          status: "idle",
          createdAt: TS,
          updatedAt: TS,
          depth: 1,
          costUsd: 0.01
        }
      ]
    },
    rejects: {
      children: [
        { id: "sess_child", parentSessionId: "sess_1", status: "succeeded", createdAt: TS, updatedAt: TS }
      ]
    },
    because: "status"
  },
  {
    name: "sessions.listFiles",
    schema: SessionFilesResponseSchema,
    accepts: { revision: checkpoint, files: [sessionFile] },
    // The result route calls this key `path`; the public file routes call it
    // `filename`. Mixing the two projections is a live risk.
    rejects: {
      revision: checkpoint,
      files: [{ ...sessionFile, filename: undefined, path: "out/report.md" }]
    },
    because: "path"
  },
  {
    name: "sessions.fileLink",
    schema: SessionFileLinkResponseSchema,
    accepts: { url: "https://storage.test/signed", expiresInSeconds: 3600, file: sessionFile },
    // `expiresAt` is synthesised client-side; the server sends none.
    rejects: {
      url: "https://storage.test/signed",
      expiresInSeconds: 3600,
      expiresAt: TS,
      file: sessionFile
    },
    because: "expiresAt"
  },
  {
    name: "sessions.eventArchiveLink",
    schema: EventArchiveLinkResponseSchema,
    accepts: { url: "https://storage.test/signed", expiresInSeconds: 3600 },
    rejects: { url: "https://storage.test/signed", expiresInSeconds: 3600, file: sessionFile },
    because: "file"
  },
  {
    name: "sessions.listWebhookDeliveries",
    schema: SessionWebhookDeliveriesResponseSchema,
    accepts: { deliveries: [{ ...delivery, lastStatusCode: 200 }] },
    // The workspace-scoped view adds these two; the session-scoped one must not.
    rejects: { deliveries: [{ ...delivery, sessionId: "sess_1", callbackUrl: "https://x/y" }] },
    because: "sessionId"
  },
  {
    name: "sessions.redeliverWebhook",
    schema: AcknowledgedResponseSchema,
    accepts: { ok: true },
    rejects: { ok: true, queued: true },
    because: "queued"
  },
  {
    name: "sessions.childResult",
    schema: ChildResultResponseSchema,
    accepts: {
      id: "sess_child",
      state: "finished",
      outcome: "succeeded",
      files: [
        {
          id: "file_1",
          path: "out.txt",
          checkpointId: "cp_01",
          sizeBytes: 3,
          sha256: "d".repeat(64),
          contentType: "text/plain"
        }
      ],
      text: "done"
    },
    rejects: { id: "sess_child", state: "done", files: [] },
    because: "state"
  },
  {
    name: "sessions.finalize",
    schema: ChildFinalizeResponseSchema,
    accepts: {
      id: "sess_child",
      lifecycleStatus: "idle",
      outcome: "succeeded",
      checkpoint,
      childCostUsd: 0.02,
      providerUsage: []
    },
    rejects: { id: "sess_child", lifecycleStatus: "idle", outcome: "succeeded", providerUsage: [] },
    because: "childCostUsd"
  },
  {
    name: "sessions.otel (traces)",
    schema: SessionOtlpResponseSchema,
    accepts: {
      resourceSpans: [
        {
          resource: { attributes: [{ key: "service.name", value: { stringValue: "aex" } }] },
          scopeSpans: [
            {
              scope: { name: "@aexhq/contracts", version: "1" },
              spans: [
                {
                  traceId: "e".repeat(32),
                  spanId: "f".repeat(16),
                  name: "RUN_STARTED",
                  kind: 1,
                  startTimeUnixNano: "1",
                  endTimeUnixNano: "2",
                  attributes: [],
                  status: { code: 1 }
                }
              ]
            }
          ]
        }
      ]
    },
    // A body carrying both roots — or an aex cursor smuggled in beside them —
    // is no longer standards-pure.
    rejects: { resourceSpans: [], nextCursor: "eyJ2IjoxfQ" },
    because: "resourceSpans"
  },
  {
    name: "whoami",
    schema: WhoAmIResponseSchema,
    accepts: whoami,
    // The exact field `parseWhoAmI` names by hand. Strictness generalises it.
    rejects: { ...whoami, caps: { sessions: true } },
    because: "caps"
  },
  {
    name: "whoami (control plane account token)",
    schema: AccountWhoAmIResponseSchema,
    accepts: { ok: true, principalType: "account_token", appUserId: "usr_1", scopes: [] },
    rejects: { ok: true, principalType: "api_key", appUserId: "usr_1", scopes: [] },
    because: "principalType"
  },
  {
    name: "assets.presign (dedup hit)",
    schema: AssetPresignResponseSchema,
    accepts: {
      ok: true,
      exists: true,
      assetId: `asset_${"c".repeat(64)}`,
      contentHash: `sha256:${"c".repeat(64)}`,
      sizeBytes: 10,
      contentType: "text/plain",
      storagePath: "workspaces/w/assets/content/cc"
    },
    // A mixture of two branches: a dedup hit that also hands out an upload URL.
    rejects: {
      ok: true,
      exists: true,
      assetId: `asset_${"c".repeat(64)}`,
      contentHash: `sha256:${"c".repeat(64)}`,
      sizeBytes: 10,
      contentType: "text/plain",
      storagePath: "workspaces/w/assets/content/cc",
      uploadUrl: "https://storage.test/put"
    },
    because: "presign"
  },
  {
    name: "assets.presign (multipart plan)",
    schema: AssetPresignResponseSchema,
    accepts: {
      ok: true,
      exists: false,
      assetId: `asset_${"c".repeat(64)}`,
      contentHash: `sha256:${"c".repeat(64)}`,
      sizeBytes: 100_000_000,
      multipart: {
        uploadId: "mpu_1",
        key: "workspaces/w/assets/content/cc",
        partSize: 8_388_608,
        partCount: 12,
        partUrls: [{ partNumber: 1, url: "https://storage.test/part1" }],
        expiresInSeconds: 3600
      }
    },
    rejects: {
      ok: true,
      exists: false,
      assetId: `asset_${"c".repeat(64)}`,
      contentHash: `sha256:${"c".repeat(64)}`,
      sizeBytes: 100_000_000,
      multipart: { uploadId: "mpu_1", key: "k", partSize: 8_388_608, partCount: 12, partUrls: [] }
    },
    because: "presign"
  },
  {
    name: "assets.finalize",
    schema: AssetFinalizeResponseSchema,
    accepts: {
      ok: true,
      exists: true,
      assetId: `asset_${"c".repeat(64)}`,
      contentHash: `sha256:${"c".repeat(64)}`,
      sizeBytes: 10,
      contentType: "text/plain"
    },
    rejects: {
      ok: true,
      exists: false,
      assetId: `asset_${"c".repeat(64)}`,
      contentHash: `sha256:${"c".repeat(64)}`,
      sizeBytes: 10,
      contentType: "text/plain"
    },
    because: "exists"
  },
  {
    name: "assets.mpuPresignParts",
    schema: AssetMpuPresignPartsResponseSchema,
    accepts: {
      ok: true,
      partUrls: [{ partNumber: 2, url: "https://storage.test/part2" }],
      expiresInSeconds: 3600
    },
    rejects: { ok: true, partUrls: [{ partNumber: 0, url: "https://storage.test/part0" }], expiresInSeconds: 3600 },
    because: "partNumber"
  },
  {
    name: "assets.mpuAbort",
    schema: AssetMpuAbortResponseSchema,
    accepts: { ok: true },
    rejects: { ok: false },
    because: "ok"
  },
  {
    name: "204 (assets.delete, workspace.*.delete, secrets.delete, mcpServers.delete)",
    schema: NoContentResponseSchema,
    accepts: {},
    rejects: { ok: true },
    because: "ok"
  },
  {
    name: "workspace.files.get",
    schema: WorkspaceFileResponseSchema,
    accepts: { resource: { kind: "file", ...workspaceResourceCommon, mountPath: "/workspace/notes.md" } },
    // A skill's metadata on a file record — the generic handler makes this a
    // real failure mode, not a hypothetical.
    rejects: { resource: { kind: "file", ...workspaceResourceCommon, description: "notes" } },
    because: "description"
  },
  {
    name: "workspace.skills.get",
    schema: WorkspaceSkillResponseSchema,
    accepts: { resource: { kind: "skill", ...workspaceResourceCommon, description: "does a thing" } },
    rejects: { resource: { kind: "skill", ...workspaceResourceCommon } },
    because: "description"
  },
  {
    name: "workspace.tools.get",
    schema: WorkspaceToolResponseSchema,
    accepts: {
      resource: {
        kind: "tool",
        ...workspaceResourceCommon,
        description: "does a thing",
        entry: "index.mjs",
        input_schema: { type: "object", properties: {} }
      }
    },
    rejects: {
      resource: {
        kind: "tool",
        ...workspaceResourceCommon,
        description: "does a thing",
        entry: "index.mjs",
        input_schema: "object"
      }
    },
    because: "input_schema"
  },
  {
    name: "workspace.instructions.get",
    schema: WorkspaceInstructionResponseSchema,
    accepts: { resource: { kind: "instruction", ...workspaceInstructionRecord } },
    // The retired asset pair, not a foreign kind's metadata: an instruction
    // record that still names bytes in the asset store is the exact drift this
    // schema exists to reject.
    rejects: {
      resource: {
        kind: "instruction",
        ...workspaceInstructionRecord,
        assetId: `asset_${"c".repeat(64)}`
      }
    },
    because: "assetId"
  },
  {
    name: "workspace.files.list",
    schema: WorkspaceFilePageResponseSchema,
    accepts: {
      resources: [{ kind: "file", ...workspaceResourceCommon, mountPath: "/workspace/notes.md" }]
    },
    rejects: {
      resources: [{ kind: "file", ...workspaceResourceCommon, mountPath: "/workspace/notes.md" }],
      total: 1
    },
    because: "total"
  },
  {
    name: "workspaces.erase",
    schema: WorkspaceEraseResponseSchema,
    accepts: {
      ok: true,
      workspaceId: "wsp_abc",
      erased: {
        workspaceId: "wsp_abc",
        sessionsErased: 0,
        deletedObjects: 0,
        deletedConnections: 0,
        deletedEgressRows: 0,
        deletedEgressPolicies: 0
      }
    },
    rejects: { ok: true, workspaceId: "wsp_abc", erased: { workspaceId: "wsp_abc", sessionsErased: 0 } },
    because: "deletedObjects"
  },
  {
    name: "secrets.get",
    schema: SecretResponseSchema,
    accepts: { secret },
    // The whole point of a metadata-only read.
    rejects: { secret: { ...secret, value: "sk-live-abc" } },
    because: "value"
  },
  {
    name: "secrets.list",
    schema: SecretListResponseSchema,
    accepts: { secrets: [secret] },
    rejects: { secrets: [{ ...secret, state: "deleted" }] },
    because: "state"
  },
  {
    name: "mcpServers.get",
    schema: McpServerResponseSchema,
    accepts: { mcpServer },
    // Header NAMES are public; header values are not.
    rejects: { mcpServer: { ...mcpServer, headers: { authorization: "Bearer x" } } },
    because: "headers"
  },
  {
    name: "mcpServers.list",
    schema: McpServerListResponseSchema,
    accepts: { mcpServers: [mcpServer] },
    rejects: { mcpServers: [{ ...mcpServer, headerShape: "authorization" }] },
    because: "headerShape"
  },
  {
    name: "billing.get",
    schema: BillingSummaryResponseSchema,
    accepts: billingSummary,
    // The retired catalog envelope. `planKey`/`subscriptionStatus`/`pastDueAt`
    // describe a subscription that no longer exists, and the prepaid fields that
    // replaced them are absent — so the whole body is refused, not tolerated.
    rejects: {
      balanceUsd: 5,
      monthSpendUsd: 1.25,
      spendCapUsd: 0,
      planKey: "free",
      subscriptionStatus: "none",
      paymentMethodStatus: "none",
      accountType: "standard",
      pastDueAt: null
    },
    because: "planKey"
  },
  {
    name: "billing.autoTopup",
    schema: BillingAutoTopupResponseSchema,
    accepts: { autoTopup: billingSummary.autoTopup },
    // A settings echo without the guards is a form with no way to know the
    // minimum it must enforce.
    rejects: { autoTopup: { enabled: false, thresholdUsd: 5, amountUsd: 20 } },
    because: "minimumAmountUsd"
  },
  {
    name: "billing.ledger",
    schema: BillingLedgerResponseSchema,
    accepts: {
      entries: [
        {
          id: "led_1",
          entryType: "usage_debit",
          amountUsd: -0.0123,
          currency: "USD",
          sessionId: "sess_1",
          workspaceId: "0f8f-uuid",
          description: null,
          createdBy: "settle",
          createdAt: "2026-07-25 12:00:00"
        }
      ]
    },
    rejects: {
      entries: [
        {
          id: "led_1",
          entryType: "usage_debit",
          amountUsd: "-0.0123",
          currency: "USD",
          sessionId: null,
          workspaceId: null,
          description: null,
          createdBy: "settle",
          createdAt: "2026-07-25 12:00:00"
        }
      ]
    },
    because: "amountUsd"
  },
  {
    name: "billing.topupCheckout / billing.portal",
    schema: BillingHostedSessionResponseSchema,
    accepts: { url: "https://checkout.stripe.com/c/pay/cs_test" },
    rejects: { url: "https://checkout.stripe.com/c/pay/cs_test", sessionId: "cs_test" },
    because: "sessionId"
  },
  {
    name: "adminBilling.topup",
    schema: AdminBillingTopupResponseSchema,
    accepts: {
      ok: true,
      workspaceId: "wsp_abc",
      orgId: "org_1",
      amountUsd: 25,
      inserted: true,
      balanceUsd: 30
    },
    rejects: { ok: true, workspaceId: "wsp_abc", orgId: "org_1", amountUsd: 25, balanceUsd: 30 },
    because: "inserted"
  },
  {
    name: "adminBilling.paymentMethod",
    schema: AdminBillingPaymentMethodResponseSchema,
    accepts: { ok: true, workspaceId: "wsp_abc", paymentMethodStatus: "active" },
    rejects: { ok: true, workspaceId: "wsp_abc", paymentMethodStatus: "trialing" },
    because: "paymentMethodStatus"
  },
  {
    name: "adminBilling.accountType",
    schema: AdminBillingAccountTypeResponseSchema,
    accepts: { ok: true, workspaceId: "wsp_abc", accountType: "internal" },
    rejects: { ok: true, workspaceId: "wsp_abc", accountType: "enterprise" },
    because: "accountType"
  },
  {
    name: "webhook.signingSecret",
    schema: WebhookSigningSecretResponseSchema,
    accepts: { whsec: "whsec_YWJjZGVm" },
    rejects: { whsec: "whsec_YWJjZGVm", rotatedAt: TS },
    because: "rotatedAt"
  },
  {
    name: "webhook.listDeliveries",
    schema: WorkspaceWebhookDeliveriesResponseSchema,
    accepts: {
      deliveries: [{ ...delivery, sessionId: "sess_1", callbackUrl: "https://hooks.test/aex" }]
    },
    // The workspace view without its workspace-view keys is the session view,
    // served at the wrong route.
    rejects: { deliveries: [delivery] },
    because: "sessionId"
  }
];

describe("response schemas", () => {
  it.each(cases.map((entry) => [entry.name, entry] as const))(
    "%s accepts a realistic response",
    (_name, entry) => {
      const result = validate(entry.schema, entry.accepts);
      expect(result.issues?.map((issue) => issue.message) ?? []).toEqual([]);
    }
  );

  it.each(cases.map((entry) => [entry.name, entry] as const))(
    "%s rejects a plausible violation",
    (_name, entry) => {
      const result = validate(entry.schema, entry.rejects);
      expect(result.issues).toBeDefined();
      const rendered = (result.issues ?? [])
        .map((issue) => `${(issue.path ?? []).join(".")} ${issue.message}`)
        .join(" | ");
      expect(rendered).toContain(entry.because);
    }
  );

  it("names the offending field and the declared set on an undeclared key", () => {
    const result = validate(WhoAmIResponseSchema, { ...whoami, tokenName: "ci" });
    const message = result.issues?.[0]?.message ?? "";
    expect(message).toContain("response.tokenName is not a declared response field");
    expect(message).toContain("declared: ok, principalType, workspaceId, scopes, limits");
  });

  it("anchors a nested issue on the response root, not on a doubled segment", () => {
    const result = validate(SessionEnvelopeResponseSchema, {
      session: { ...sessionWire, acceptsMessages: "yes" }
    });
    expect(result.issues?.[0]?.message).toBe("response.session.acceptsMessages must be a boolean");
  });

  it("rejects a non-object response body", () => {
    const result = validate(SessionEnvelopeResponseSchema, "not json");
    expect(result.issues?.[0]?.message).toBe("response must be an object");
  });

  it("validates synchronously — the harness cannot await an observer", () => {
    for (const entry of cases) {
      expect(entry.schema["~standard"].validate(entry.accepts)).not.toBeInstanceOf(Promise);
    }
  });

  it("carries its metadata OUTSIDE z.globalRegistry", () => {
    // `scripts/openapi/registry.ts` sets OPENAPI_SCHEMA_REGISTRY = z.globalRegistry
    // and the generator converts every id in it. A response schema registered
    // there would add an unreferenced component to the committed
    // openapi/data-plane.json and fail C3 (spec freshness) as a side effect of
    // importing this file. `responseSchemaRegistry` exists to keep that from
    // being a thing anyone has to remember.
    for (const entry of cases) {
      expect(z.globalRegistry.has(entry.schema as never)).toBe(false);
    }
    expect(responseSchemaRegistry.get(WhoAmIResponseSchema)?.id).toBe("WhoAmIResponse");
  });
});
