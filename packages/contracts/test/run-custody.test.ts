import { describe, expect, it } from "vitest";
import {
  CUSTODY_MANIFEST_SCHEMA_VERSION,
  CUSTODY_TOMBSTONE_SCHEMA_VERSION,
  CustodyManifestRedactionError,
  FakeCustodyManifestObjectStore,
  buildCustodyManifest,
  buildCustodyTombstoneFromManifest,
  createCustodyManifestWriter,
  custodyManifestObjectKey,
  scanCustodyPayloadForSensitiveValues
} from "../src/index.js";

const baseRun = {
  runId: "run-11111111",
  workspaceId: "workspace-11111111",
  provider: "anthropic",
  runtime: "native",
  terminalStatus: "succeeded",
  credentialMode: "byok",
  createdAt: "2026-06-02T10:00:00.000Z",
  terminalAt: "2026-06-02T10:05:00.000Z"
} as const;

describe("run custody manifest contract", () => {
  it("builds a metadata-only terminal custody manifest", () => {
    const manifest = buildCustodyManifest({
      generatedAt: "2026-06-02T10:05:01.000Z",
      finalizedAt: "2026-06-02T10:05:02.000Z",
      run: baseRun,
      secrets: [
        {
          class: "provider_api_key",
          present: true,
          count: 1,
          exposures: [
            {
              surface: "aex_vault",
              access: "stored",
              status: "revoked",
              firstExposedAt: "2026-06-02T10:00:00.000Z",
              revokedAt: "2026-06-02T10:05:01.000Z"
            },
            {
              surface: "provider_session",
              access: "replicated",
              status: "revoked",
              firstExposedAt: "2026-06-02T10:00:10.000Z",
              revokedAt: "2026-06-02T10:05:01.000Z"
            }
          ],
          disposition: {
            status: "destroyed",
            decidedAt: "2026-06-02T10:05:01.000Z"
          },
          evidence: [
            {
              source: "cleanup_step",
              status: "confirmed",
              observedAt: "2026-06-02T10:05:01.000Z",
              count: 1
            }
          ]
        },
        {
          class: "runner_bearer",
          present: true,
          exposures: [{ surface: "aex_kv", access: "stored", status: "revoked" }],
          disposition: { status: "revoked", decidedAt: "2026-06-02T10:05:01.000Z" }
        }
      ],
      resources: [
        {
          class: "native_provider_session",
          count: 1,
          exposures: [{ surface: "provider_session", access: "replicated", status: "revoked" }],
          disposition: {
            status: "provider_delete_confirmed",
            decidedAt: "2026-06-02T10:05:02.000Z"
          },
          evidence: [{ source: "provider_cleanup_summary", status: "confirmed", count: 1 }]
        },
        {
          class: "run_output",
          count: 2,
          exposures: [{ surface: "run_artifact_store", access: "stored", status: "retained" }],
          disposition: {
            status: "retained_by_policy",
            reason: "run_retained_until_user_delete",
            decidedAt: "2026-06-02T10:05:02.000Z"
          }
        }
      ],
      cleanup: {
        status: "partial",
        startedAt: "2026-06-02T10:05:00.000Z",
        finishedAt: "2026-06-02T10:05:02.000Z"
      }
    });

    expect(manifest.schemaVersion).toBe(CUSTODY_MANIFEST_SCHEMA_VERSION);
    expect(manifest.run).toMatchObject({
      runId: "run-11111111",
      workspaceId: "workspace-11111111",
      provider: "anthropic",
      runtime: "native",
      terminalStatus: "succeeded"
    });
    expect(manifest.summary).toMatchObject({
      secretClassCount: 2,
      secretInstanceCount: 2,
      resourceClassCount: 2,
      resourceInstanceCount: 3,
      exposureCount: 5,
      revokedExposureCount: 4,
      retainedCount: 1
    });
    expect(manifest.redaction.excludes).toEqual([
      "raw_secret_values",
      "bearer_hashes",
      "provider_response_bodies",
      "signed_urls",
      "object_store_keys",
      "vault_ids",
      "private_resource_handles"
    ]);
    expect(scanCustodyPayloadForSensitiveValues(manifest)).toEqual([]);
    expect(JSON.parse(JSON.stringify(manifest))).toEqual(manifest);
  });

  it("rejects secret values, private handles, signed URLs, object-store keys, vault ids, and forbidden fields", () => {
    const cases: readonly [string, unknown, string][] = [
      ["provider key", "sk-ant-test-1234567890", "provider_key"],
      ["bearer", "Bearer runner-token-1234567890", "bearer_token"],
      ["signed URL", "https://object-storage.example.test/file?X-Amz-Signature=abc", "signed_url"],
      ["object-store key", "runs/run-11111111/metadata/custody.json", "object_store_key"],
      ["vault id", "vault_secret_1234567890", "vault_id"],
      ["resource handle", "session_1234567890", "private_resource_handle"],
      ["forbidden field", { vaultId: "redacted" }, "forbidden_field_name"]
    ];

    for (const [name, payload, reason] of cases) {
      const findings = scanCustodyPayloadForSensitiveValues(payload);
      expect(findings, name).toEqual([
        expect.objectContaining({ reason })
      ]);
    }

    expect(() =>
      buildCustodyManifest({
        generatedAt: "2026-06-02T10:05:01.000Z",
        run: baseRun,
        secrets: [
          {
            class: "provider_api_key",
            present: true,
            exposures: [],
            disposition: {
              status: "cleanup_failed",
              errorClass: "sk-ant-test-1234567890"
            }
          }
        ]
      })
    ).toThrow(CustodyManifestRedactionError);
  });

  it("writes through the public writer interface without embedding object keys in the manifest", async () => {
    const store = new FakeCustodyManifestObjectStore();
    const writer = createCustodyManifestWriter(store);

    const result = await writer.writeCustodyManifest({
      generatedAt: "2026-06-02T10:05:01.000Z",
      run: baseRun,
      secrets: [
        {
          class: "mcp_credential",
          present: false,
          exposures: [],
          disposition: { status: "not_applicable" }
        }
      ],
      cleanup: { status: "succeeded", finishedAt: "2026-06-02T10:05:01.000Z" }
    });

    expect(result).toMatchObject({
      status: "written",
      runId: "run-11111111",
      workspaceId: "workspace-11111111",
      key: custodyManifestObjectKey("run-11111111")
    });
    expect(store.listKeys()).toEqual([custodyManifestObjectKey("run-11111111")]);

    const stored = store.getByRunId("run-11111111");
    expect(stored?.summary.secretInstanceCount).toBe(0);
    expect(JSON.stringify(stored)).not.toContain("runs/run-11111111/");
    expect(scanCustodyPayloadForSensitiveValues(stored)).toEqual([]);
  });

  it("does NOT flag content-addressed hashes, URLs, /workspace paths, or tool-result text (over-masking fix)", () => {
    const sha256 = "abf1471e1a247d1839f66d723e15b46456ea30926d36a7a37a2e12bdb4787deb";
    // 2 KB of legit web-search result text (the broll false-positive class):
    // long underscore-joined URL slugs + prose, NOT a secret.
    const searchText =
      "1. Watch ted Season 2, Episode 4: The Mom's Bombed Rom-Com\n" +
      "https://www.reddit.com/r/television/comments/1rma2ro/ted_season_2_peacock_official_discussion_thread/\n" +
      "ted, The Mom's Bombed Rom-Com. Season 2, Episode 4. Blaire's rom-com movie marathon annoys Matty.\n".repeat(20);

    const survivors: ReadonlyArray<readonly [string, unknown]> = [
      ["sha256 content hash", sha256],
      ["sha256 asset filename path", `/workspace/files/asset_${sha256}/source-video-subtitles-srt`],
      ["manifest filename field", { files: [{ path: `outputs/${sha256}`, filename: sha256 }], outputs: [{ filename: sha256 }] }],
      ["canonical uuid", "9728bf4e-9711-4e6f-9152-15d6a9c70578"],
      ["normal https url", "https://www.reddit.com/r/television/comments/1rma2ro/ted_season_2_peacock_official_discussion_thread/"],
      ["web_fetch url argument", { data: { arguments: { url: "https://en.wikipedia.org/wiki/Norah_Jones?oldid=123456789" } } }],
      ["2KB search-result text block", { data: { content: [{ type: "text", text: searchText }] } }]
    ];

    for (const [name, payload] of survivors) {
      expect(scanCustodyPayloadForSensitiveValues(payload), name).toEqual([]);
    }
  });

  it("STILL flags real provider keys, bearers, and opaque secret blobs (no weakening)", () => {
    const sk = scanCustodyPayloadForSensitiveValues("sk-1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d");
    expect(sk.map((f) => f.reason)).toContain("provider_key"); // bare DeepSeek-style sk- key

    const skAnt = scanCustodyPayloadForSensitiveValues("sk-ant-api03-aBcD1234efGh5678ijKlmnop");
    expect(skAnt.map((f) => f.reason)).toContain("provider_key");

    const blob = scanCustodyPayloadForSensitiveValues("Zx9Kq2Lp7Vn4Rt6Wy8Ub3Mc5Ad1Ef0Gh2Ij4Kl6mnopQRstuv99");
    expect(blob.map((f) => f.reason)).toContain("high_entropy_token"); // opaque alnum-mixed secret

    // The catch-all still fires when a content-hash-LENGTH-mismatched opaque run
    // (here 50 chars, not a 32/40/64 digest) is alnum-mixed and high-entropy —
    // the exemption is strictly the canonical hash lengths, nothing longer.
    const nonHashBlob = scanCustodyPayloadForSensitiveValues("aGVsbG8td29ybGQtMTIzNDU2Nzg5MEFCQ0RFRkdISUpLTE1OT1A");
    expect(nonHashBlob.map((f) => f.reason)).toContain("high_entropy_token");

    // Regression for the existing keyword/handle patterns (untouched by the fix).
    const handle = scanCustodyPayloadForSensitiveValues("session_1234567890abcdef");
    expect(handle.map((f) => f.reason)).toContain("private_resource_handle");
  });

  it("builds an indefinite-retention tombstone with only identity, counts, statuses, and timestamps", () => {
    const manifest = buildCustodyManifest({
      generatedAt: "2026-06-02T10:05:01.000Z",
      finalizedAt: "2026-06-02T10:05:02.000Z",
      run: baseRun,
      secrets: [
        {
          class: "provider_api_key",
          present: true,
          exposures: [{ surface: "aex_vault", access: "stored", status: "revoked" }],
          disposition: { status: "destroyed", decidedAt: "2026-06-02T10:05:01.000Z" }
        }
      ],
      resources: [
        {
          class: "run_output",
          count: 2,
          exposures: [{ surface: "run_artifact_store", access: "stored", status: "retained" }],
          disposition: { status: "retained_by_policy", decidedAt: "2026-06-02T10:05:02.000Z" }
        }
      ]
    });

    const tombstone = buildCustodyTombstoneFromManifest(manifest, {
      manifestStatus: "purged",
      tombstonedAt: "2026-06-02T10:06:00.000Z",
      deletion: {
        status: "deleted",
        pendingAt: "2026-06-02T10:05:30.000Z",
        deletedAt: "2026-06-02T10:06:00.000Z"
      }
    });

    expect(tombstone.schemaVersion).toBe(CUSTODY_TOMBSTONE_SCHEMA_VERSION);
    expect(tombstone).toMatchObject({
      run: {
        runId: "run-11111111",
        workspaceId: "workspace-11111111",
        terminalStatus: "succeeded",
        terminalAt: "2026-06-02T10:05:00.000Z"
      },
      manifest: {
        schemaVersion: CUSTODY_MANIFEST_SCHEMA_VERSION,
        status: "purged",
        tombstonedAt: "2026-06-02T10:06:00.000Z"
      },
      retention: {
        defaultPolicy: "retain_indefinitely",
        userAction: "purge_or_anonymize_later"
      }
    });
    expect(tombstone.summary).toMatchObject({
      secretClassCount: 1,
      resourceClassCount: 1,
      exposureCount: 2,
      retainedCount: 1
    });

    const serialized = JSON.stringify(tombstone);
    expect(serialized).not.toContain("anthropic");
    expect(serialized).not.toContain("native");
    expect(serialized).not.toContain("provider_api_key");
    expect(serialized).not.toContain("run_artifact_store");
    expect(scanCustodyPayloadForSensitiveValues(tombstone)).toEqual([]);
  });
});
