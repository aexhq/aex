import { describe, expect, it } from "vitest";
import {
  SIDE_EFFECT_AUDIT_ACTIONS,
  SIDE_EFFECT_AUDIT_KIND,
  SIDE_EFFECT_AUDIT_SCHEMA_VERSION,
  SideEffectAuditRedactionError,
  buildCustodyManifestWrittenAuditEvent,
  buildRunDeletionCompletedAuditEvent,
  buildRunDeletionFailedAuditEvent,
  buildRunDeletionRequestedAuditEvent,
  buildRunDownloadRequestedAuditEvent,
  buildSideEffectAuditEvent,
  redactSideEffectAuditMetadata,
  scanSideEffectAuditPayloadForSensitiveValues
} from "../src/index.js";

const actor = {
  principal: { type: "api_token", ref: "api-token-11111111" },
  sourcePlane: "worker",
  authenticatedBy: "api_token"
} as const;

describe("side-effect audit contract", () => {
  it("builds a public-safe side-effect audit event with actor, target, run, correlation, counts, status, and timestamps", () => {
    const event = buildSideEffectAuditEvent({
      auditId: "audit-11111111",
      workspaceId: "workspace-11111111",
      runId: "run-11111111",
      action: "proxy.endpoint.called",
      outcome: "succeeded",
      observedAt: "2026-06-02T12:00:00.000Z",
      actor,
      target: {
        type: "proxy_endpoint",
        id: "proxy-call-11111111",
        name: "httpbin"
      },
      correlation: {
        requestId: "req-11111111",
        operationId: "op-11111111",
        eventId: "evt-11111111",
        idempotencyKeyRef: "idem-ref-11111111"
      },
      metadata: {
        status: {
          status: "ok",
          statusCode: 200
        },
        counts: {
          requestBytes: 128,
          responseBytes: 256,
          durationMs: 42,
          proxyCallCount: 1
        },
        timestamps: {
          startedAt: "2026-06-02T11:59:59.000Z",
          finishedAt: "2026-06-02T12:00:00.000Z"
        },
        dimensions: {
          runtime: "managed",
          provider: "anthropic",
          credentialMode: "byok",
          method: "GET",
          surface: "named_proxy_endpoint"
        }
      }
    });

    expect(event.schemaVersion).toBe(SIDE_EFFECT_AUDIT_SCHEMA_VERSION);
    expect(event.kind).toBe(SIDE_EFFECT_AUDIT_KIND);
    expect(event.actor).toMatchObject({
      principal: { type: "api_token", ref: "api-token-11111111" },
      sourcePlane: "worker"
    });
    expect(event.target).toMatchObject({
      type: "proxy_endpoint",
      name: "httpbin"
    });
    expect(event.metadata.counts?.responseBytes).toBe(256);
    expect(event.metadata.redaction.excludes).toContain("provider_response_bodies");
    expect(scanSideEffectAuditPayloadForSensitiveValues(event)).toEqual([]);
    expect(JSON.parse(JSON.stringify(event))).toEqual(event);
  });

  it("rejects unsupported actions, actor identities, and unsafe identifiers", () => {
    expect(() =>
      buildSideEffectAuditEvent({
        workspaceId: "workspace-11111111",
        action: "agent.session.started" as never,
        outcome: "succeeded",
        observedAt: "2026-06-02T12:00:00.000Z",
        actor,
        target: { type: "run" }
      })
    ).toThrow(/action agent\.session\.started is not supported/);

    expect(() =>
      buildSideEffectAuditEvent({
        workspaceId: "workspace-11111111",
        action: "run.cancel.requested",
        outcome: "accepted",
        observedAt: "2026-06-02T12:00:00.000Z",
        actor: {
          principal: { type: "agent" as never, ref: "agent-11111111" },
          sourcePlane: "runtime"
        },
        target: { type: "run" }
      })
    ).toThrow(/principal type agent is not supported/);

    expect(() =>
      buildSideEffectAuditEvent({
        workspaceId: "workspace-11111111",
        action: "run.cancel.requested",
        outcome: "accepted",
        observedAt: "2026-06-02T12:00:00.000Z",
        actor: {
          principal: { type: "runtime", ref: "session-11111111" },
          sourcePlane: "runtime"
        },
        target: { type: "run" }
      })
    ).toThrow(/must not introduce agent, session, or customer identity/);

    expect(() =>
      buildSideEffectAuditEvent({
        workspaceId: "workspace-11111111",
        runId: "runs/run-11111111",
        action: "run.cancel.requested",
        outcome: "accepted",
        observedAt: "2026-06-02T12:00:00.000Z",
        actor,
        target: { type: "run" }
      })
    ).toThrow(/runId must be an opaque identifier/);
  });

  it("builds API-token deletion audit events", () => {
    expect(SIDE_EFFECT_AUDIT_ACTIONS).toContain("api_token.deleted");

    const event = buildSideEffectAuditEvent({
      workspaceId: "workspace-11111111",
      action: "api_token.deleted",
      outcome: "succeeded",
      observedAt: "2026-06-02T12:00:00.000Z",
      actor,
      target: { type: "api_token", id: "api-token-11111111" },
      metadata: {
        status: { status: "deleted" },
        timestamps: { deletedAt: "2026-06-02T12:00:00.000Z" }
      }
    });

    expect(event).toMatchObject({
      action: "api_token.deleted",
      outcome: "succeeded",
      target: { type: "api_token", id: "api-token-11111111" },
      metadata: { status: { status: "deleted" } }
    });
    expect(scanSideEffectAuditPayloadForSensitiveValues(event)).toEqual([]);
  });

  it("rejects headers, bodies, raw URLs, raw paths, provider details, secret values, and private handles", () => {
    const cases: readonly [string, unknown, string][] = [
      ["headers", { headers: { authorization: "Bearer runner-token-1234567890" } }, "forbidden_field_name"],
      ["provider key", "sk-ant-test-1234567890", "provider_key"],
      ["signed URL", "https://object-storage.example.test/file?X-Amz-Signature=abc", "signed_url"],
      ["object-store key", "runs/run-11111111/outputs/result.txt", "object_store_key"],
      ["Vault id", "vault_secret_1234567890", "vault_id"],
      ["resource handle", "machine_1234567890", "private_resource_handle"],
      ["raw URL", "https://service.example.test/path", "raw_url"],
      ["raw path", "/api/runs/run-11111111/proxy/httpbin?token=secret", "raw_path"],
      ["provider account", { providerAccountId: "acct_private" }, "forbidden_field_name"],
      ["customer identity", { customerId: "customer-11111111" }, "forbidden_field_name"]
    ];

    for (const [name, payload, reason] of cases) {
      expect(scanSideEffectAuditPayloadForSensitiveValues(payload), name).toEqual(expect.arrayContaining([
        expect.objectContaining({ reason })
      ]));
    }

    expect(() =>
      buildSideEffectAuditEvent({
        workspaceId: "workspace-11111111",
        runId: "run-11111111",
        action: "mcp.proxy.called",
        outcome: "failed",
        observedAt: "2026-06-02T12:00:00.000Z",
        actor,
        target: { type: "mcp_proxy", name: "github" },
        metadata: {
          status: {
            errorClass: "Bearer runner-token-1234567890"
          }
        }
      })
    ).toThrow(SideEffectAuditRedactionError);
  });

  it("keeps deletion metadata limited to counts, status, and timestamps", () => {
    const metadata = redactSideEffectAuditMetadata(
      {
        status: { status: "deleted" },
        counts: {
          deletedObjectCount: 4,
          retainedObjectCount: 1,
          failedObjectCount: 0
        },
        timestamps: {
          deletedAt: "2026-06-02T12:00:00.000Z"
        }
      },
      "run.delete.completed"
    );

    expect(metadata).toMatchObject({
      status: { status: "deleted" },
      counts: {
        deletedObjectCount: 4,
        retainedObjectCount: 1,
        failedObjectCount: 0
      },
      timestamps: {
        deletedAt: "2026-06-02T12:00:00.000Z"
      }
    });
    expect(metadata).not.toHaveProperty("dimensions");

    expect(() =>
      redactSideEffectAuditMetadata(
        {
          status: { status: "deleted" },
          dimensions: {
            provider: "anthropic"
          }
        },
        "run.delete.completed"
      )
    ).toThrow(/metadata\.dimensions is not supported/);
  });

  it("builds deletion lifecycle audit events with counts, statuses, and timestamps only", () => {
    const requested = buildRunDeletionRequestedAuditEvent({
      workspaceId: "workspace-11111111",
      runId: "run-11111111",
      observedAt: "2026-06-02T12:00:00.000Z",
      actor,
      metadata: {
        status: { status: "delete_requested" },
        timestamps: { observedAt: "2026-06-02T12:00:00.000Z" }
      }
    });
    const completed = buildRunDeletionCompletedAuditEvent({
      workspaceId: "workspace-11111111",
      runId: "run-11111111",
      observedAt: "2026-06-02T12:00:05.000Z",
      actor,
      metadata: {
        status: { status: "deleted" },
        counts: {
          deletedObjectCount: 8,
          retainedObjectCount: 1,
          failedObjectCount: 0
        },
        timestamps: {
          deletedAt: "2026-06-02T12:00:04.000Z"
        }
      }
    });
    const failed = buildRunDeletionFailedAuditEvent({
      workspaceId: "workspace-11111111",
      runId: "run-11111111",
      observedAt: "2026-06-02T12:00:03.000Z",
      actor,
      metadata: {
        status: { status: "delete_failed", errorClass: "deletion_purge_failed" },
        counts: { failedObjectCount: 1 },
        timestamps: { observedAt: "2026-06-02T12:00:03.000Z" }
      }
    });

    expect(requested).toMatchObject({
      action: "run.delete.requested",
      outcome: "accepted",
      target: { type: "deletion", id: "run-11111111" }
    });
    expect(completed).toMatchObject({
      action: "run.delete.completed",
      outcome: "succeeded",
      metadata: {
        counts: { deletedObjectCount: 8, retainedObjectCount: 1, failedObjectCount: 0 }
      }
    });
    expect(failed).toMatchObject({
      action: "run.delete.failed",
      outcome: "failed",
      metadata: { status: { errorClass: "deletion_purge_failed" } }
    });
    for (const event of [requested, completed, failed]) {
      expect(event.metadata).not.toHaveProperty("dimensions");
      expect(scanSideEffectAuditPayloadForSensitiveValues(event)).toEqual([]);
    }

    expect(() =>
      buildRunDeletionCompletedAuditEvent({
        workspaceId: "workspace-11111111",
        runId: "run-11111111",
        observedAt: "2026-06-02T12:00:05.000Z",
        actor,
        metadata: {
          status: { status: "deleted" },
          dimensions: { surface: "worker_delete_route" }
        }
      })
    ).toThrow(/metadata\.dimensions is not supported/);
  });

  it("builds download and custody audit events without raw locations or secret custody values", () => {
    const download = buildRunDownloadRequestedAuditEvent({
      workspaceId: "workspace-11111111",
      runId: "run-11111111",
      observedAt: "2026-06-02T12:00:00.000Z",
      actor,
      metadata: {
        counts: { outputCount: 2, logCount: 1, eventCount: 3 },
        dimensions: { namespace: "archive", method: "GET", surface: "sdk_zip_download" }
      }
    });
    const custody = buildCustodyManifestWrittenAuditEvent({
      workspaceId: "workspace-11111111",
      runId: "run-11111111",
      observedAt: "2026-06-02T12:00:01.000Z",
      actor,
      metadata: {
        status: { status: "written" },
        counts: {
          secretClassCount: 2,
          resourceClassCount: 4
        },
        timestamps: { observedAt: "2026-06-02T12:00:01.000Z" }
      }
    });

    expect(download).toMatchObject({
      action: "run.download.requested",
      outcome: "accepted",
      target: { type: "output_archive", id: "run-11111111" },
      metadata: { dimensions: { namespace: "archive", method: "GET" } }
    });
    expect(custody).toMatchObject({
      action: "custody.manifest.written",
      outcome: "succeeded",
      target: { type: "custody_manifest", id: "run-11111111" }
    });
    expect(custody.metadata.redaction.excludes).toContain("vault_ids");
    expect(scanSideEffectAuditPayloadForSensitiveValues(download)).toEqual([]);
    expect(scanSideEffectAuditPayloadForSensitiveValues(custody)).toEqual([]);

    expect(() =>
      buildRunDownloadRequestedAuditEvent({
        workspaceId: "workspace-11111111",
        runId: "run-11111111",
        observedAt: "2026-06-02T12:00:00.000Z",
        actor,
        metadata: {
          status: { status: "https://object-storage.example.test/file?X-Amz-Signature=abc" }
        }
      })
    ).toThrow(SideEffectAuditRedactionError);

    expect(() =>
      buildCustodyManifestWrittenAuditEvent({
        workspaceId: "workspace-11111111",
        runId: "run-11111111",
        observedAt: "2026-06-02T12:00:01.000Z",
        actor,
        metadata: {
          status: { status: "vault_secret_1234567890" }
        }
      })
    ).toThrow(SideEffectAuditRedactionError);
  });

  it("rejects unsupported metadata fields and invalid counts or timestamps", () => {
    expect(() =>
      redactSideEffectAuditMetadata({
        status: { status: "ok", providerResponseBody: "redacted" } as never
      })
    ).toThrow(SideEffectAuditRedactionError);

    expect(() =>
      redactSideEffectAuditMetadata({
        counts: { outputCount: -1 }
      })
    ).toThrow(/outputCount must be a non-negative finite number/);

    expect(() =>
      redactSideEffectAuditMetadata({
        timestamps: { observedAt: "not-a-date" }
      })
    ).toThrow(/observedAt must be an ISO timestamp string/);
  });
});
