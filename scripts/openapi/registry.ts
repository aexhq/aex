/**
 * OpenAPI component registry for the accepted v1 wire.
 *
 * Every entry is the same strict schema used by the public contracts package.
 * Removed runtime/submission/checkpoint/archive schemas have no registry entry.
 */
import { z } from "zod";
import {
  ApiErrorSchema,
  EffectiveWorkspaceLimitSchema,
  MessageSchema,
  MessageSendRequestSchema,
  OperationSchema,
  RunSchema,
  SessionCreateRequestSchema,
  SessionSchema,
  PageSchema,
  WorkspaceApiKeyValueSchema
} from "../../packages/contracts/src/v1-resources.js";
import {
  ApprovalResponseRequestSchema,
  BlobDescriptorSchema,
  DownloadGrantSchema,
  FileDownloadRequestSchema,
  LiveFileDownloadRequestSchema,
  LiveFileListRequestSchema,
  LiveFileStatRequestSchema,
  PersistedFileListRequestSchema,
  PersistedFileStatRequestSchema,
  RegisteredFileDownloadRequestSchema,
  RegisteredFileInputSchema,
  RegisteredFileValueSchema,
  RegisteredInstructionInputSchema,
  RegisteredInstructionValueSchema,
  RegisteredMcpServerInputSchema,
  RegisteredMcpServerValueSchema,
  RegisteredResourcePageSchema,
  RegisteredResourceSchema,
  RegisteredSkillInputSchema,
  RegisteredSkillValueSchema,
  RegisteredToolInputSchema,
  RegisteredToolValueSchema,
  RegistryPutResultSchema,
  SecretSetRequestSchema,
  UploadCompleteRequestSchema,
  UploadCreateRequestSchema,
  UploadPartsRequestSchema,
  UploadPartsResponseSchema,
  UploadSchema
} from "../../packages/contracts/src/v1-content.js";
import {
  MetricAggregationRequestSchema,
  ObservationListenRequestSchema,
  ObservationQuerySchema,
  ObservationStreamRequestSchema,
  TelemetryExportRequestSchema,
  TelemetryGapQuerySchema
} from "../../packages/contracts/src/v1-telemetry.js";
import {
  ApiKeyCreateRequestSchema,
  AutoTopupPolicyRequestSchema,
  InvitationCreateRequestSchema,
  OrganizationCreateRequestSchema,
  PortalSessionRequestSchema,
  TopUpCheckoutRequestSchema,
  UsageQuerySchema,
  WorkspaceCreateRequestSchema,
  WorkspaceDeleteRequestSchema
} from "../../packages/contracts/src/v1-account-billing.js";

function register<Schema extends object>(
  id: string,
  schema: Schema
): Schema {
  z.globalRegistry.add(schema as never, { id });
  return schema;
}

for (const [id, schema] of [
  ["ApiError", ApiErrorSchema],
  ["EffectiveWorkspaceLimit", EffectiveWorkspaceLimitSchema],
  ["EffectiveWorkspaceLimitPage", PageSchema(EffectiveWorkspaceLimitSchema)],
  ["Message", MessageSchema],
  ["MessageSendRequest", MessageSendRequestSchema],
  ["Operation", OperationSchema],
  ["Run", RunSchema],
  ["Session", SessionSchema],
  ["SessionCreateRequestV1", SessionCreateRequestSchema],
  ["WorkspaceApiKeyValue", WorkspaceApiKeyValueSchema],
  ["OrganizationCreateRequest", OrganizationCreateRequestSchema],
  ["InvitationCreateRequest", InvitationCreateRequestSchema],
  ["WorkspaceCreateRequest", WorkspaceCreateRequestSchema],
  ["WorkspaceDeleteRequest", WorkspaceDeleteRequestSchema],
  ["ApiKeyCreateRequest", ApiKeyCreateRequestSchema],
  ["TopUpCheckoutRequest", TopUpCheckoutRequestSchema],
  ["PortalSessionRequest", PortalSessionRequestSchema],
  ["AutoTopupPolicyRequest", AutoTopupPolicyRequestSchema],
  ["PersistedFileListRequest", PersistedFileListRequestSchema],
  ["PersistedFileStatRequest", PersistedFileStatRequestSchema],
  ["FileDownloadRequest", FileDownloadRequestSchema],
  ["LiveFileListRequest", LiveFileListRequestSchema],
  ["LiveFileStatRequest", LiveFileStatRequestSchema],
  ["LiveFileDownloadRequest", LiveFileDownloadRequestSchema],
  ["RegisteredFileDownloadRequest", RegisteredFileDownloadRequestSchema],
  ["RegisteredFileInput", RegisteredFileInputSchema],
  ["RegisteredSkillInput", RegisteredSkillInputSchema],
  ["RegisteredToolInput", RegisteredToolInputSchema],
  ["RegisteredInstructionInput", RegisteredInstructionInputSchema],
  ["RegisteredMcpServerInput", RegisteredMcpServerInputSchema],
  ["BlobDescriptor", BlobDescriptorSchema],
  ["RegisteredFileValue", RegisteredFileValueSchema],
  ["RegisteredSkillValue", RegisteredSkillValueSchema],
  ["RegisteredToolValue", RegisteredToolValueSchema],
  ["RegisteredInstructionValue", RegisteredInstructionValueSchema],
  ["RegisteredMcpServerValue", RegisteredMcpServerValueSchema],
  ["RegisteredResource", RegisteredResourceSchema],
  ["RegisteredResourcePage", RegisteredResourcePageSchema],
  ["RegistryPutResult", RegistryPutResultSchema],
  ["DownloadGrant", DownloadGrantSchema],
  ["UploadCreateRequest", UploadCreateRequestSchema],
  ["UploadPartsRequest", UploadPartsRequestSchema],
  ["UploadCompleteRequest", UploadCompleteRequestSchema],
  ["Upload", UploadSchema],
  ["UploadPartsResponse", UploadPartsResponseSchema],
  ["SecretSetRequest", SecretSetRequestSchema],
  ["ApprovalResponseRequest", ApprovalResponseRequestSchema],
  ["UsageQuery", UsageQuerySchema],
  ["ObservationQuery", ObservationQuerySchema],
  ["ObservationStreamRequest", ObservationStreamRequestSchema],
  ["ObservationListenRequest", ObservationListenRequestSchema],
  ["MetricAggregationRequest", MetricAggregationRequestSchema],
  ["TelemetryGapQuery", TelemetryGapQuerySchema],
  ["TelemetryExportRequest", TelemetryExportRequestSchema]
] as const) {
  register(id, schema);
}

export const OPENAPI_SCHEMA_REGISTRY = z.globalRegistry;

const observationBodies: Readonly<Record<string, string>> = Object.fromEntries(
  ["events", "logs", "spans", "metrics", "traces", "telemetry"].flatMap(
    (signal) => [
      [`${signal}.query`, "ObservationQuery"],
      [`${signal}.stream`, "ObservationStreamRequest"],
      [`${signal}.listen`, "ObservationListenRequest"],
      [`session.${signal}.query`, "ObservationQuery"],
      [`session.${signal}.stream`, "ObservationStreamRequest"],
      [`session.${signal}.listen`, "ObservationListenRequest"]
    ]
  )
);

export const OPENAPI_REQUEST_BODIES: Readonly<Record<string, string>> = {
  "organizations.create": "OrganizationCreateRequest",
  "invitations.create": "InvitationCreateRequest",
  "workspaces.create": "WorkspaceCreateRequest",
  "workspaces.delete": "WorkspaceDeleteRequest",
  "apiKeys.create": "ApiKeyCreateRequest",
  "billing.topUpCheckout": "TopUpCheckoutRequest",
  "billing.portalSession": "PortalSessionRequest",
  "billing.autoTopup.put": "AutoTopupPolicyRequest",
  "sessions.create": "SessionCreateRequestV1",
  "messages.send": "MessageSendRequest",
  "files.persisted.list": "PersistedFileListRequest",
  "files.persisted.stat": "PersistedFileStatRequest",
  "files.persisted.download": "FileDownloadRequest",
  "files.live.list": "LiveFileListRequest",
  "files.live.stat": "LiveFileStatRequest",
  "files.live.download": "LiveFileDownloadRequest",
  "registry.files.put": "RegisteredFileInput",
  "registry.files.download": "RegisteredFileDownloadRequest",
  "registry.skills.put": "RegisteredSkillInput",
  "registry.tools.put": "RegisteredToolInput",
  "registry.instructions.put": "RegisteredInstructionInput",
  "registry.mcpServers.put": "RegisteredMcpServerInput",
  "uploads.create": "UploadCreateRequest",
  "uploads.parts": "UploadPartsRequest",
  "uploads.complete": "UploadCompleteRequest",
  "secrets.put": "SecretSetRequest",
  "approvals.respond": "ApprovalResponseRequest",
  "billing.usage.query": "UsageQuery",
  "metrics.aggregate": "MetricAggregationRequest",
  "session.metrics.aggregate": "MetricAggregationRequest",
  "telemetry.gaps.query": "TelemetryGapQuery",
  "session.telemetry.gaps.query": "TelemetryGapQuery",
  "telemetry.exports.create": "TelemetryExportRequest",
  "session.telemetry.exports.create": "TelemetryExportRequest",
  ...observationBodies
};

const registryResponses = Object.fromEntries(
  ["files", "skills", "tools", "instructions", "mcpServers"].flatMap(
    (kind) => [
      [`registry.${kind}.list`, "RegisteredResourcePage"],
      [`registry.${kind}.get`, "RegisteredResource"],
      [`registry.${kind}.put`, "RegistryPutResult"]
    ]
  )
);

export const OPENAPI_RESPONSE_BODIES: Readonly<Record<string, string>> = {
  "workspace.limits.list": "EffectiveWorkspaceLimitPage",
  "workspace.limits.get": "EffectiveWorkspaceLimit",
  ...registryResponses,
  "registry.files.download": "DownloadGrant",
  "uploads.create": "Upload",
  "uploads.parts": "UploadPartsResponse",
  "uploads.complete": "Upload"
};
