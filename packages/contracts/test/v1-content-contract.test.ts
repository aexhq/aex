import { describe, expect, it } from "bun:test";
import {
  ApprovalResponseRequestSchema,
  ApprovalSchema,
  BlobDescriptorSchema,
  BlobInputSchema,
  DownloadGrantSchema,
  FileDownloadRequestSchema,
  FileEntrySchema,
  LiveFileDownloadRequestSchema,
  LiveFileListRequestSchema,
  RegisteredFileDownloadRequestSchema,
  RegisteredFileInputSchema,
  RegisteredResourceSchema,
  RegisteredResourceSummarySchema,
  RegistryPutResultSchema,
  REGIONAL_API_ROUTE_DESCRIPTORS,
  SecretMetadataSchema,
  SecretRevocationSchema,
  UploadCompleteRequestSchema,
  UploadCreateRequestSchema,
  UploadPartsRequestSchema,
  UploadSchema,
  newId
} from "../src/index.js";

const at = "2026-07-30T10:00:00.000Z";
const hash = `sha256:${"a".repeat(64)}`;

function accepts(schema: { safeParse(value: unknown): { success: boolean } }, value: unknown): boolean {
  return schema.safeParse(value).success;
}

describe("v1 session files and download grants", () => {
  it("pins file entries, live access controls, and half-open byte ranges", () => {
    expect(accepts(FileEntrySchema, {
      path: "reports/result.csv",
      type: "file",
      sizeBytes: 12,
      sha256: hash,
      mode: "0644",
      mtime: at
    })).toBe(true);
    expect(accepts(LiveFileListRequestSchema, {
      path: "reports",
      recursive: true,
      wake: "never",
      consistency: "best_effort",
      ifGenerationId: newId("generation")
    })).toBe(true);
    expect(accepts(FileDownloadRequestSchema, {
      path: "reports/result.csv",
      range: { start: 0, endExclusive: 12 }
    })).toBe(true);
    expect(accepts(FileDownloadRequestSchema, {
      path: "reports/result.csv",
      range: { start: 12, endExclusive: 12 }
    })).toBe(false);
    expect(accepts(LiveFileDownloadRequestSchema, {
      path: "reports/result.csv",
      range: { start: -1, endExclusive: 12 }
    })).toBe(false);
  });

  it("pins five-minute grant fields and forbids live-only access on persisted grants", () => {
    const grant = {
      url: "https://download.example.test/object",
      headers: { Range: "bytes=0-11" },
      expiresAt: "2026-07-30T10:05:00.000Z",
      sizeBytes: 12,
      authorizedBytes: 12,
      measurementId: newId("measurement"),
      sha256: hash
    };
    expect(accepts(DownloadGrantSchema, grant)).toBe(true);
    expect(accepts(DownloadGrantSchema, {
      ...grant,
      workspaceAccess: { generationId: newId("generation"), resumed: false }
    })).toBe(false);
    expect(accepts(DownloadGrantSchema, { ...grant, authorizedBytes: 13 })).toBe(false);
  });
});

describe("v1 overwrite-only registries and uploads", () => {
  it("accepts only name-addressed current resources with no public version identity", () => {
    const file = {
      kind: "file",
      name: "bootstrap.sh",
      revision: 2,
      state: "current",
      sha256: hash,
      sizeBytes: 12,
      value: {
        mountPath: "bin/bootstrap.sh",
        content: {
          sha256: hash,
          sizeBytes: 12
        },
        mediaType: "text/x-shellscript",
        mode: "0755"
      },
      createdAt: at,
      updatedAt: at
    };
    expect(accepts(RegisteredResourceSchema, file)).toBe(true);
    expect(accepts(RegisteredResourceSchema, { ...file, versionId: "version_1" })).toBe(false);
    const { value: _value, ...summary } = file;
    expect(accepts(RegisteredResourceSummarySchema, summary)).toBe(true);
    expect(accepts(RegisteredResourceSummarySchema, file)).toBe(false);
    expect(accepts(BlobInputSchema, {
      type: "upload",
      uploadId: newId("upload"),
      sha256: hash,
      sizeBytes: 12
    })).toBe(true);
    expect(accepts(BlobDescriptorSchema, {
      sha256: hash,
      sizeBytes: 12
    })).toBe(true);
    expect(accepts(BlobDescriptorSchema, {
      type: "upload",
      uploadId: newId("upload"),
      sha256: hash,
      sizeBytes: 12
    })).toBe(false);
    expect(accepts(RegisteredFileInputSchema, {
      mountPath: "bin/bootstrap.sh",
      content: {
        type: "inline",
        encoding: "utf8",
        data: "echo ready",
        sha256: hash
      },
      mediaType: "text/x-shellscript",
      mode: "0755"
    })).toBe(true);
    for (const status of ["created", "replaced", "unchanged"]) {
      expect(accepts(RegistryPutResultSchema, { status, resource: file })).toBe(true);
    }
    expect(accepts(RegistryPutResultSchema, file)).toBe(false);
    expect(accepts(RegisteredFileDownloadRequestSchema, {
      range: { start: 0, endExclusive: 12 }
    })).toBe(true);
    expect(accepts(RegisteredResourceSchema, {
      ...summary,
      kind: "mcp_server",
      name: "github",
      value: {
        url: "https://mcp.example.test",
        transport: "streamable_http",
        headers: [{
          name: "Authorization",
          secretName: "GITHUB_TOKEN",
          value: "Bearer plaintext"
        }]
      }
    })).toBe(false);
  });

  it("pins create/parts/completion upload staging without asset identities", () => {
    expect(accepts(UploadCreateRequestSchema, {
      sizeBytes: 12,
      sha256: hash,
      contentType: "application/gzip"
    })).toBe(true);
    expect(accepts(UploadPartsRequestSchema, {
      parts: [
        { partNumber: 1, sizeBytes: 6, sha256: hash },
        { partNumber: 2, sizeBytes: 6, sha256: hash }
      ]
    })).toBe(true);
    expect(accepts(UploadPartsRequestSchema, {
      parts: [
        { partNumber: 1, sizeBytes: 6, sha256: hash },
        { partNumber: 1, sizeBytes: 6, sha256: hash }
      ]
    })).toBe(false);
    expect(accepts(UploadCompleteRequestSchema, {
      parts: [{
        partNumber: 1,
        etag: "\"etag\"",
        sizeBytes: 12,
        sha256: hash
      }]
    })).toBe(true);
    expect(accepts(UploadCompleteRequestSchema, {
      parts: [{
        partNumber: 2,
        etag: "\"etag\"",
        sizeBytes: 12,
        sha256: hash
      }]
    })).toBe(false);
    expect(accepts(UploadSchema, {
      id: newId("upload"),
      state: "ready",
      sizeBytes: 12,
      sha256: hash,
      contentType: "application/gzip",
      createdAt: at,
      expiresAt: "2026-07-31T10:00:00.000Z"
    })).toBe(true);
  });
});

describe("v1 secrets and exact-call approvals", () => {
  it("keeps secret responses metadata-only and models explicit revocation", () => {
    const metadata = {
      name: "GITHUB_TOKEN",
      revision: 3,
      state: "revoked",
      createdAt: at,
      updatedAt: at,
      revokedAt: at
    };
    expect(accepts(SecretMetadataSchema, metadata)).toBe(true);
    expect(accepts(SecretMetadataSchema, { ...metadata, value: "plaintext" })).toBe(false);
    expect(accepts(SecretRevocationSchema, {
      name: "GITHUB_TOKEN",
      revision: 3,
      revokedAt: at
    })).toBe(true);
  });

  it("binds approval to one exact call and one winning decision", () => {
    const approval = {
      id: newId("approval"),
      sessionId: newId("session"),
      runId: newId("run"),
      agentId: newId("agent"),
      toolCallId: newId("toolCall"),
      toolName: "github.create_issue",
      argumentsSha256: hash,
      implementationSha256: hash,
      expectedGenerationId: newId("generation"),
      status: "pending",
      requestedAt: at,
      updatedAt: at
    };
    expect(accepts(ApprovalSchema, approval)).toBe(true);
    expect(accepts(ApprovalSchema, { ...approval, arguments: { title: "secret" } })).toBe(false);
    expect(accepts(ApprovalResponseRequestSchema, { decision: "approve" })).toBe(true);
    expect(accepts(ApprovalResponseRequestSchema, { decision: "allow_all" })).toBe(false);
  });
});

describe("v1 content route authority", () => {
  it("pins files, registries, uploads, secrets, and approvals with exact replay policy", () => {
    const names = new Set([
      "files.persisted.list",
      "files.persisted.stat",
      "files.persisted.download",
      "files.live.list",
      "files.live.stat",
      "files.live.download",
      "registry.files.list",
      "registry.files.get",
      "registry.files.put",
      "registry.files.delete",
      "registry.files.download",
      "uploads.create",
      "uploads.parts",
      "uploads.complete",
      "uploads.abort",
      "secrets.list",
      "secrets.get",
      "secrets.put",
      "secrets.delete",
      "secrets.revoke",
      "approvals.list",
      "approvals.get",
      "approvals.respond"
    ]);
    expect(REGIONAL_API_ROUTE_DESCRIPTORS
      .filter(({ name }) => names.has(name))
      .map(({ name, method, requiredScope, idempotency }) => [
        name, method, requiredScope, idempotency
      ])).toEqual([
      ["files.persisted.list", "POST", "files:read", "none"],
      ["files.persisted.stat", "POST", "files:read", "none"],
      ["files.persisted.download", "POST", "files:read", "idempotency-key"],
      ["files.live.list", "POST", "files:live", "none"],
      ["files.live.stat", "POST", "files:live", "none"],
      ["files.live.download", "POST", "files:live", "idempotency-key"],
      ["registry.files.list", "GET", "resources:read", "none"],
      ["registry.files.get", "GET", "resources:read", "none"],
      ["registry.files.put", "PUT", "resources:write", "idempotency-key"],
      ["registry.files.delete", "DELETE", "resources:write", "none"],
      ["registry.files.download", "POST", "resources:read", "idempotency-key"],
      ["uploads.create", "POST", "resources:write", "idempotency-key"],
      ["uploads.parts", "POST", "resources:write", "none"],
      ["uploads.complete", "POST", "resources:write", "idempotency-key"],
      ["uploads.abort", "DELETE", "resources:write", "none"],
      ["secrets.list", "GET", "secrets:read", "none"],
      ["secrets.get", "GET", "secrets:read", "none"],
      ["secrets.put", "PUT", "secrets:write", "idempotency-key"],
      ["secrets.delete", "DELETE", "secrets:write", "none"],
      ["secrets.revoke", "POST", "secrets:revoke", "idempotency-key"],
      ["approvals.list", "GET", "sessions:read", "none"],
      ["approvals.get", "GET", "sessions:read", "none"],
      ["approvals.respond", "POST", "sessions:write", "none"]
    ]);
  });

  it("has no asset/version/publish/copy/checkpoint/archive or blanket approval route", () => {
    const routes = REGIONAL_API_ROUTE_DESCRIPTORS
      .map(({ name, samplePath }) => `${name} ${samplePath}`)
      .join("\n");
    for (const removed of [
      "/assets",
      "/versions",
      "/publish",
      "/copy",
      "/checkpoint",
      "/capture",
      "/archive",
      "/restore",
      "/request-approval",
      "/approve",
      "/deny"
    ]) {
      expect(routes).not.toContain(removed);
    }
  });
});
