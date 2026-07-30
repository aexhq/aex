/**
 * OpenAPI component registry for the accepted v1 wire.
 *
 * Every entry is the same strict schema used by the public contracts package.
 * Removed runtime/submission/checkpoint/archive schemas have no registry entry.
 */
import { z } from "zod";
import {
  ApiErrorSchema,
  MessageSchema,
  MessageSendRequestSchema,
  OperationSchema,
  RunSchema,
  SessionCreateRequestSchema,
  SessionSchema,
  WorkspaceApiKeyValueSchema
} from "../../packages/contracts/src/v1-resources.js";
import {
  ApprovalResponseRequestSchema,
  FileDownloadRequestSchema,
  LiveFileDownloadRequestSchema,
  LiveFileListRequestSchema,
  LiveFileStatRequestSchema,
  PersistedFileListRequestSchema,
  PersistedFileStatRequestSchema,
  RegisteredFileValueSchema,
  RegisteredInstructionValueSchema,
  RegisteredMcpServerValueSchema,
  RegisteredSkillValueSchema,
  RegisteredToolValueSchema,
  SecretSetRequestSchema,
  UploadCompleteRequestSchema,
  UploadCreateRequestSchema,
  UploadPartsRequestSchema
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
  ["RegisteredFileValue", RegisteredFileValueSchema],
  ["RegisteredSkillValue", RegisteredSkillValueSchema],
  ["RegisteredToolValue", RegisteredToolValueSchema],
  ["RegisteredInstructionValue", RegisteredInstructionValueSchema],
  ["RegisteredMcpServerValue", RegisteredMcpServerValueSchema],
  ["UploadCreateRequest", UploadCreateRequestSchema],
  ["UploadPartsRequest", UploadPartsRequestSchema],
  ["UploadCompleteRequest", UploadCompleteRequestSchema],
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
  "registry.files.put": "RegisteredFileValue",
  "registry.skills.put": "RegisteredSkillValue",
  "registry.tools.put": "RegisteredToolValue",
  "registry.instructions.put": "RegisteredInstructionValue",
  "registry.mcpServers.put": "RegisteredMcpServerValue",
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
