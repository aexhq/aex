import { describe, expect, it } from "vitest";
import {
  RUN_RETENTION_SCHEMA_VERSION,
  RunRetentionRedactionError,
  RunRetentionValidationError,
  FakeRunDeletionManifestObjectStore,
  assertRunDeletionOrder,
  buildRunDeletionJob,
  buildRunDeletionManifest,
  buildRunRetentionPolicy,
  createRunDeletionManifestWriter,
  evaluateRunDeletionCandidate,
  scanRunRetentionPayloadForSensitiveValues
} from "../src/index.js";

const terminalRun = {
  runId: "run-11111111",
  workspaceId: "workspace-11111111",
  status: "succeeded",
  createdAt: "2026-06-01T10:00:00.000Z",
  terminalAt: "2026-06-01T10:05:00.000Z"
} as const;

const finalManifest = {
  status: "written",
  mode: "final",
  writtenAt: "2026-06-02T10:00:00.000Z"
} as const;

const completedPurge = {
  status: "completed",
  startedAt: "2026-06-02T10:00:01.000Z",
  completedAt: "2026-06-02T10:00:05.000Z",
  deletedObjectCount: 8
} as const;

describe("run retention and deletion contract", () => {
  it("defaults to indefinite retention and rejects negative or implicit retention TTLs", () => {
    expect(buildRunRetentionPolicy()).toEqual({
      mode: "retain_indefinitely",
      manualDelete: "enabled",
      automaticDeletion: "disabled"
    });

    expect(() => buildRunRetentionPolicy({ automaticDeletion: true })).toThrow(
      RunRetentionValidationError
    );
    expect(() => buildRunRetentionPolicy({ automaticDeletion: true, retentionDays: 0 })).toThrow(
      RunRetentionValidationError
    );
    expect(() => buildRunRetentionPolicy({ automaticDeletion: true, retentionDays: -1 })).toThrow(
      RunRetentionValidationError
    );
    expect(() => buildRunRetentionPolicy({ retentionDays: 30 })).toThrow(
      RunRetentionValidationError
    );
    expect(() =>
      evaluateRunDeletionCandidate({
        run: terminalRun,
        reason: "retention_gc",
        policy: {
          mode: "retain_indefinitely",
          manualDelete: "enabled",
          automaticDeletion: "disabled",
          retentionDays: 30
        } as never,
        now: "2026-06-02T10:00:00.000Z"
      })
    ).toThrow(RunRetentionValidationError);

    expect(buildRunRetentionPolicy({ automaticDeletion: true, retentionDays: 30 })).toEqual({
      mode: "delete_after_days",
      manualDelete: "enabled",
      automaticDeletion: "enabled",
      retentionDays: 30
    });
  });

  it("selects manual deletion for eligible terminal runs and blocks default automatic GC", () => {
    expect(
      evaluateRunDeletionCandidate({
        run: terminalRun,
        reason: "manual_delete",
        now: "2026-06-02T10:00:00.000Z"
      })
    ).toMatchObject({
      status: "selected",
      reason: "manual_delete",
      eligibleAt: "2026-06-02T10:00:00.000Z",
      blockers: []
    });

    expect(
      evaluateRunDeletionCandidate({
        run: terminalRun,
        reason: "retention_gc",
        now: "2026-06-02T10:00:00.000Z"
      })
    ).toMatchObject({
      status: "blocked",
      reason: "retention_gc",
      blockers: [{ code: "retention_policy_disabled" }]
    });
  });

  it("requires enabled retention policy and elapsed cutoff before automatic GC selects a run", () => {
    const policy = buildRunRetentionPolicy({ automaticDeletion: true, retentionDays: 7 });

    expect(
      evaluateRunDeletionCandidate({
        run: terminalRun,
        reason: "retention_gc",
        policy,
        now: "2026-06-02T10:00:00.000Z"
      })
    ).toMatchObject({
      status: "blocked",
      eligibleAt: "2026-06-08T10:05:00.000Z",
      blockers: [{ code: "unexpired" }]
    });

    expect(
      evaluateRunDeletionCandidate({
        run: terminalRun,
        reason: "retention_gc",
        policy,
        now: "2026-06-08T10:05:00.000Z"
      })
    ).toMatchObject({
      status: "selected",
      eligibleAt: "2026-06-08T10:05:00.000Z",
      blockers: []
    });
  });

  it("records blockers for non-terminal, held, exempt, unresolved, and pending-delete runs", () => {
    const candidate = evaluateRunDeletionCandidate({
      run: {
        ...terminalRun,
        status: "pending_delete",
        held: true,
        retentionExempt: true,
        unresolvedCleanup: true,
        unresolvedCustody: true
      },
      reason: "manual_delete",
      now: "2026-06-02T10:00:00.000Z"
    });

    expect(candidate.status).toBe("blocked");
    expect(candidate.blockers.map((entry) => entry.code)).toEqual([
      "concurrent_delete",
      "held",
      "retention_exempt",
      "unresolved_cleanup",
      "unresolved_custody"
    ]);

    expect(
      evaluateRunDeletionCandidate({
        run: { ...terminalRun, status: "provider_running" },
        reason: "manual_delete",
        now: "2026-06-02T10:00:00.000Z"
      }).blockers
    ).toEqual([{ code: "non_terminal", observedAt: "2026-06-02T10:00:00.000Z" }]);
  });

  it("builds a public-safe manifest with counts, statuses, and timestamps only", () => {
    const manifest = buildRunDeletionManifest({
      generatedAt: "2026-06-02T10:00:00.000Z",
      mode: "final",
      run: terminalRun,
      request: {
        reason: "manual_delete",
        actorClass: "user"
      },
      counts: [
        {
          class: "object_store_objects",
          count: 8,
          status: "counted",
          countedAt: "2026-06-02T09:59:59.000Z"
        },
        {
          class: "outputs",
          count: 2,
          status: "counted"
        },
        {
          class: "storage_samples",
          count: 1,
          status: "partial",
          errorClass: "storage_accounting_lag"
        }
      ]
    });

    expect(manifest.schemaVersion).toBe(RUN_RETENTION_SCHEMA_VERSION);
    expect(manifest.run.eligibleAt).toBe("2026-06-02T10:00:00.000Z");
    expect(manifest.summary).toMatchObject({
      totalCount: 11,
      failedCountClasses: 0,
      partialCountClasses: 1,
      blockerCount: 0
    });
    expect(manifest.redaction).toMatchObject({
      policy: "counts_status_timestamps_only",
      excludes: [
        "raw_paths",
        "object_keys",
        "filenames",
        "object_sizes",
        "hashes",
        "provider_ids",
        "vault_ids",
        "resource_ids",
        "resource_handles",
        "signed_urls"
      ]
    });
    expect(scanRunRetentionPayloadForSensitiveValues(manifest)).toEqual([]);

    const serialized = JSON.stringify(manifest);
    expect(serialized).not.toContain("runs/run-11111111");
  });

  it("rejects paths, object keys, filenames, sizes, hashes, provider ids, Vault ids, handles, and signed URLs", () => {
    const cases: readonly [string, unknown, string][] = [
      ["path field", { path: "redacted" }, "forbidden_field_name"],
      ["object key", "runs/run-11111111/outputs/result.txt", "object_store_key"],
      ["filename field", { filename: "result.txt" }, "forbidden_field_name"],
      ["size field", { size: 10 }, "forbidden_field_name"],
      ["hash field", { hash: "sha256:abcdef1234567890" }, "forbidden_field_name"],
      ["provider id field", { providerId: "session_1234567890" }, "forbidden_field_name"],
      ["Vault id", "vault_secret_1234567890", "vault_id"],
      ["handle", "machine_1234567890", "private_resource_handle"],
      ["signed URL", "https://object-storage.example.test/file?X-Amz-Signature=abc", "signed_url"]
    ];

    for (const [name, payload, reason] of cases) {
      expect(scanRunRetentionPayloadForSensitiveValues(payload), name).toEqual(
        expect.arrayContaining([expect.objectContaining({ reason })])
      );
    }

    expect(() =>
      buildRunDeletionManifest({
        generatedAt: "2026-06-02T10:00:00.000Z",
        mode: "final",
        run: terminalRun,
        request: { reason: "manual_delete", actorClass: "user" },
        counts: [{ class: "outputs", count: 1, status: "failed", errorClass: "runs/run-11111111/output.txt" }]
      })
    ).toThrow(RunRetentionRedactionError);
  });

  it("writes manifests through the public writer without embedding object keys in the manifest", async () => {
    const store = new FakeRunDeletionManifestObjectStore();
    const writer = createRunDeletionManifestWriter(store);

    const result = await writer.writeRunDeletionManifest({
      generatedAt: "2026-06-02T10:00:00.000Z",
      mode: "dry_run",
      run: terminalRun,
      request: { reason: "manual_delete", actorClass: "api_token" },
      counts: [{ class: "events", count: 4, status: "counted" }]
    });

    expect(result).toMatchObject({
      status: "written",
      runId: "run-11111111",
      workspaceId: "workspace-11111111",
      mode: "dry_run"
    });
    expect(store.listRunIds()).toEqual(["run-11111111"]);
    expect(JSON.stringify(store.getByRunId("run-11111111"))).not.toContain("runs/run-11111111/");
  });

  it("makes deletion ordering explicit: final manifest proof before purge", () => {
    expect(() =>
      assertRunDeletionOrder({
        manifest: { status: "not_written" },
        purge: { status: "running", startedAt: "2026-06-02T10:00:00.000Z" }
      })
    ).toThrow(/manifest is written/);

    expect(() =>
      assertRunDeletionOrder({
        manifest: { status: "written", mode: "dry_run", writtenAt: "2026-06-02T10:00:00.000Z" },
        purge: { status: "running", startedAt: "2026-06-02T10:00:01.000Z" }
      })
    ).toThrow(/dry-run/);

    expect(() =>
      assertRunDeletionOrder({
        manifest: finalManifest,
        purge: completedPurge
      })
    ).not.toThrow();
  });

  it("builds a deletion job status shape bound by the same order invariant", () => {
    expect(
      buildRunDeletionJob({
        jobId: "delete-job-11111111",
        runId: "run-11111111",
        workspaceId: "workspace-11111111",
        reason: "manual_delete",
        mode: "final",
        status: "completed",
        createdAt: "2026-06-02T09:58:00.000Z",
        updatedAt: "2026-06-02T10:00:06.000Z",
        order: {
          manifest: finalManifest,
          purge: completedPurge
        }
      })
    ).toMatchObject({
      schemaVersion: RUN_RETENTION_SCHEMA_VERSION,
      jobId: "delete-job-11111111",
      runId: "run-11111111",
      status: "completed"
    });

    expect(() =>
      buildRunDeletionJob({
        jobId: "delete-job-11111111",
        runId: "run-11111111",
        workspaceId: "workspace-11111111",
        reason: "manual_delete",
        mode: "final",
        status: "completed",
        createdAt: "2026-06-02T09:58:00.000Z",
        order: {
          manifest: finalManifest,
          purge: { status: "not_started" }
        }
      })
    ).not.toThrow();
  });
});
