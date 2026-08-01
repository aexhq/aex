---
# GENERATED FILE - do not edit.
# Written by `scripts/docs/generate-all.mjs`.
# Edit the source and run `bun run docs:generate`. A hand edit here is
# reverted by the next `bun run lint`, which regenerates via `prelint`.
title: "HTTP API"
description: "Generated HTTP API reference for the aex data plane."
---

# HTTP API

Generated from `packages/contracts/openapi/regional.json` — the OpenAPI 3.1.0 document that
`bun run openapi:generate` derives from the request schemas and route table in
`@aexhq/contracts`. The document itself ships in the published package, so a
non-TypeScript client can read this surface from the same source this page does.

Version `0.33.0`. Regenerate this page with `bun run docs:generate`.

## Base URLs

| Environment | Base URL |
| --- | --- |
| us-east-1 | `https://us-east-1.api.aex.dev` |
| us-east-2 | `https://us-east-2.api.aex.dev` |
| us-west-2 | `https://us-west-2.api.aex.dev` |
| ap-northeast-1 | `https://ap-northeast-1.api.aex.dev` |
| eu-west-1 | `https://eu-west-1.api.aex.dev` |

Every path below is relative to a base URL and already carries its `/api` prefix.

## Authentication

Region-pinned workspace API key, sent as `Authorization: Bearer <key>`.

The **Scope** column below names the API key scope a call requires.

## Responses

21 of 113 operations describe their success body. The rest return a body the document cannot yet describe, not an empty one.

## Routes

113 operations across 99 paths.

62 of 113 operations describe their request body. An operation showing `—` accepts a body the document cannot yet describe, not a body it refuses.

### Approvals

| Method | Path | Operation | Scope | Request body |
| --- | --- | --- | --- | --- |
| `GET` | `/api/sessions/{sessionId}/approvals` | `approvals.list` | `sessions:read` | — |
| `GET` | `/api/sessions/{sessionId}/approvals/{approvalId}` | `approvals.get` | `sessions:read` | — |
| `POST` | `/api/sessions/{sessionId}/approvals/{approvalId}/responses` | `approvals.respond` | `sessions:write` | [`ApprovalResponseRequest`](#approvalresponserequest) |

### Billing

| Method | Path | Operation | Scope | Request body |
| --- | --- | --- | --- | --- |
| `POST` | `/api/billing/usage/query` | `billing.usage.query` | `billing:read` | [`UsageQuery`](#usagequery) |

### Events

| Method | Path | Operation | Scope | Request body |
| --- | --- | --- | --- | --- |
| `POST` | `/api/events/listen` | `events.listen` | `telemetry:read` | [`ObservationListenRequest`](#observationlistenrequest) |
| `POST` | `/api/events/query` | `events.query` | `telemetry:read` | [`ObservationQuery`](#observationquery) |
| `POST` | `/api/events/stream` | `events.stream` | `telemetry:read` | [`ObservationStreamRequest`](#observationstreamrequest) |

### Files

| Method | Path | Operation | Scope | Request body |
| --- | --- | --- | --- | --- |
| `POST` | `/api/sessions/{sessionId}/files/live/downloads` | `files.live.download` | `files:live` | [`LiveFileDownloadRequest`](#livefiledownloadrequest) |
| `POST` | `/api/sessions/{sessionId}/files/live/list` | `files.live.list` | `files:live` | [`LiveFileListRequest`](#livefilelistrequest) |
| `POST` | `/api/sessions/{sessionId}/files/live/stat` | `files.live.stat` | `files:live` | [`LiveFileStatRequest`](#livefilestatrequest) |
| `POST` | `/api/sessions/{sessionId}/files/persisted/downloads` | `files.persisted.download` | `files:read` | [`FileDownloadRequest`](#filedownloadrequest) |
| `POST` | `/api/sessions/{sessionId}/files/persisted/list` | `files.persisted.list` | `files:read` | [`PersistedFileListRequest`](#persistedfilelistrequest) |
| `POST` | `/api/sessions/{sessionId}/files/persisted/stat` | `files.persisted.stat` | `files:read` | [`PersistedFileStatRequest`](#persistedfilestatrequest) |

### Logs

| Method | Path | Operation | Scope | Request body |
| --- | --- | --- | --- | --- |
| `POST` | `/api/logs/listen` | `logs.listen` | `telemetry:read` | [`ObservationListenRequest`](#observationlistenrequest) |
| `POST` | `/api/logs/query` | `logs.query` | `telemetry:read` | [`ObservationQuery`](#observationquery) |
| `POST` | `/api/logs/stream` | `logs.stream` | `telemetry:read` | [`ObservationStreamRequest`](#observationstreamrequest) |

### Messages

| Method | Path | Operation | Scope | Request body |
| --- | --- | --- | --- | --- |
| `GET` | `/api/sessions/{sessionId}/messages` | `messages.list` | `sessions:read` | — |
| `POST` | `/api/sessions/{sessionId}/messages` | `messages.send` | `sessions:write` | [`MessageSendRequest`](#messagesendrequest) |

### Metrics

| Method | Path | Operation | Scope | Request body |
| --- | --- | --- | --- | --- |
| `POST` | `/api/metrics/aggregate` | `metrics.aggregate` | `telemetry:read` | [`MetricAggregationRequest`](#metricaggregationrequest) |
| `POST` | `/api/metrics/listen` | `metrics.listen` | `telemetry:read` | [`ObservationListenRequest`](#observationlistenrequest) |
| `POST` | `/api/metrics/query` | `metrics.query` | `telemetry:read` | [`ObservationQuery`](#observationquery) |
| `POST` | `/api/metrics/stream` | `metrics.stream` | `telemetry:read` | [`ObservationStreamRequest`](#observationstreamrequest) |

### Operations

| Method | Path | Operation | Scope | Request body |
| --- | --- | --- | --- | --- |
| `GET` | `/api/operations` | `operations.list` | `operations:read` | — |
| `GET` | `/api/operations/{operationId}` | `operations.get` | `operations:read` | — |
| `POST` | `/api/operations/{operationId}/cancellations` | `operations.cancel` | `operations:write` | — |

### Registry

| Method | Path | Operation | Scope | Request body |
| --- | --- | --- | --- | --- |
| `GET` | `/api/workspace/files` | `registry.files.list` | `resources:read` | — |
| `GET` | `/api/workspace/files/{name}` | `registry.files.get` | `resources:read` | — |
| `PUT` | `/api/workspace/files/{name}` | `registry.files.put` | `resources:write` | [`RegisteredFileInput`](#registeredfileinput) |
| `DELETE` | `/api/workspace/files/{name}` | `registry.files.delete` | `resources:write` | — |
| `POST` | `/api/workspace/files/{name}/downloads` | `registry.files.download` | `resources:read` | [`RegisteredFileDownloadRequest`](#registeredfiledownloadrequest) |
| `GET` | `/api/workspace/instructions` | `registry.instructions.list` | `resources:read` | — |
| `GET` | `/api/workspace/instructions/{name}` | `registry.instructions.get` | `resources:read` | — |
| `PUT` | `/api/workspace/instructions/{name}` | `registry.instructions.put` | `resources:write` | [`RegisteredInstructionInput`](#registeredinstructioninput) |
| `DELETE` | `/api/workspace/instructions/{name}` | `registry.instructions.delete` | `resources:write` | — |
| `GET` | `/api/workspace/mcp-servers` | `registry.mcpServers.list` | `resources:read` | — |
| `GET` | `/api/workspace/mcp-servers/{name}` | `registry.mcpServers.get` | `resources:read` | — |
| `PUT` | `/api/workspace/mcp-servers/{name}` | `registry.mcpServers.put` | `resources:write` | [`RegisteredMcpServerInput`](#registeredmcpserverinput) |
| `DELETE` | `/api/workspace/mcp-servers/{name}` | `registry.mcpServers.delete` | `resources:write` | — |
| `GET` | `/api/workspace/skills` | `registry.skills.list` | `resources:read` | — |
| `GET` | `/api/workspace/skills/{name}` | `registry.skills.get` | `resources:read` | — |
| `PUT` | `/api/workspace/skills/{name}` | `registry.skills.put` | `resources:write` | [`RegisteredSkillInput`](#registeredskillinput) |
| `DELETE` | `/api/workspace/skills/{name}` | `registry.skills.delete` | `resources:write` | — |
| `GET` | `/api/workspace/tools` | `registry.tools.list` | `resources:read` | — |
| `GET` | `/api/workspace/tools/{name}` | `registry.tools.get` | `resources:read` | — |
| `PUT` | `/api/workspace/tools/{name}` | `registry.tools.put` | `resources:write` | [`RegisteredToolInput`](#registeredtoolinput) |
| `DELETE` | `/api/workspace/tools/{name}` | `registry.tools.delete` | `resources:write` | — |

### Runs

| Method | Path | Operation | Scope | Request body |
| --- | --- | --- | --- | --- |
| `GET` | `/api/sessions/{sessionId}/runs` | `runs.list` | `sessions:read` | — |
| `GET` | `/api/sessions/{sessionId}/runs/{runId}` | `runs.get` | `sessions:read` | — |

### Secrets

| Method | Path | Operation | Scope | Request body |
| --- | --- | --- | --- | --- |
| `GET` | `/api/workspace/secrets` | `secrets.list` | `secrets:read` | — |
| `GET` | `/api/workspace/secrets/{secretId}` | `secrets.get` | `secrets:read` | — |
| `PUT` | `/api/workspace/secrets/{secretId}` | `secrets.put` | `secrets:write` | [`SecretSetRequest`](#secretsetrequest) |
| `DELETE` | `/api/workspace/secrets/{secretId}` | `secrets.delete` | `secrets:write` | — |
| `POST` | `/api/workspace/secrets/{secretId}/revocations` | `secrets.revoke` | `secrets:revoke` | — |

### Session

| Method | Path | Operation | Scope | Request body |
| --- | --- | --- | --- | --- |
| `POST` | `/api/sessions/{sessionId}/events/listen` | `session.events.listen` | `telemetry:read` | [`ObservationListenRequest`](#observationlistenrequest) |
| `POST` | `/api/sessions/{sessionId}/events/query` | `session.events.query` | `telemetry:read` | [`ObservationQuery`](#observationquery) |
| `POST` | `/api/sessions/{sessionId}/events/stream` | `session.events.stream` | `telemetry:read` | [`ObservationStreamRequest`](#observationstreamrequest) |
| `POST` | `/api/sessions/{sessionId}/logs/listen` | `session.logs.listen` | `telemetry:read` | [`ObservationListenRequest`](#observationlistenrequest) |
| `POST` | `/api/sessions/{sessionId}/logs/query` | `session.logs.query` | `telemetry:read` | [`ObservationQuery`](#observationquery) |
| `POST` | `/api/sessions/{sessionId}/logs/stream` | `session.logs.stream` | `telemetry:read` | [`ObservationStreamRequest`](#observationstreamrequest) |
| `POST` | `/api/sessions/{sessionId}/metrics/aggregate` | `session.metrics.aggregate` | `telemetry:read` | [`MetricAggregationRequest`](#metricaggregationrequest) |
| `POST` | `/api/sessions/{sessionId}/metrics/listen` | `session.metrics.listen` | `telemetry:read` | [`ObservationListenRequest`](#observationlistenrequest) |
| `POST` | `/api/sessions/{sessionId}/metrics/query` | `session.metrics.query` | `telemetry:read` | [`ObservationQuery`](#observationquery) |
| `POST` | `/api/sessions/{sessionId}/metrics/stream` | `session.metrics.stream` | `telemetry:read` | [`ObservationStreamRequest`](#observationstreamrequest) |
| `POST` | `/api/sessions/{sessionId}/spans/listen` | `session.spans.listen` | `telemetry:read` | [`ObservationListenRequest`](#observationlistenrequest) |
| `POST` | `/api/sessions/{sessionId}/spans/query` | `session.spans.query` | `telemetry:read` | [`ObservationQuery`](#observationquery) |
| `POST` | `/api/sessions/{sessionId}/spans/stream` | `session.spans.stream` | `telemetry:read` | [`ObservationStreamRequest`](#observationstreamrequest) |
| `POST` | `/api/sessions/{sessionId}/telemetry/exports` | `session.telemetry.exports.create` | `telemetry:read` | [`TelemetryExportRequest`](#telemetryexportrequest) |
| `GET` | `/api/sessions/{sessionId}/telemetry/exports/{exportId}` | `session.telemetry.exports.get` | `telemetry:read` | — |
| `POST` | `/api/sessions/{sessionId}/telemetry/exports/{exportId}/downloads` | `session.telemetry.exports.download` | `telemetry:read` | — |
| `POST` | `/api/sessions/{sessionId}/telemetry/exports/{exportId}/revocations` | `session.telemetry.exports.revoke` | `telemetry:read` | — |
| `GET` | `/api/sessions/{sessionId}/telemetry/gaps/{gapId}` | `session.telemetry.gaps.get` | `telemetry:read` | — |
| `POST` | `/api/sessions/{sessionId}/telemetry/gaps/query` | `session.telemetry.gaps.query` | `telemetry:read` | [`TelemetryGapQuery`](#telemetrygapquery) |
| `POST` | `/api/sessions/{sessionId}/telemetry/listen` | `session.telemetry.listen` | `telemetry:read` | [`ObservationListenRequest`](#observationlistenrequest) |
| `POST` | `/api/sessions/{sessionId}/telemetry/query` | `session.telemetry.query` | `telemetry:read` | [`ObservationQuery`](#observationquery) |
| `POST` | `/api/sessions/{sessionId}/telemetry/stream` | `session.telemetry.stream` | `telemetry:read` | [`ObservationStreamRequest`](#observationstreamrequest) |
| `GET` | `/api/sessions/{sessionId}/traces/{traceId}` | `session.traces.get` | `telemetry:read` | — |
| `POST` | `/api/sessions/{sessionId}/traces/listen` | `session.traces.listen` | `telemetry:read` | [`ObservationListenRequest`](#observationlistenrequest) |
| `POST` | `/api/sessions/{sessionId}/traces/query` | `session.traces.query` | `telemetry:read` | [`ObservationQuery`](#observationquery) |
| `POST` | `/api/sessions/{sessionId}/traces/stream` | `session.traces.stream` | `telemetry:read` | [`ObservationStreamRequest`](#observationstreamrequest) |

### Sessions

| Method | Path | Operation | Scope | Request body |
| --- | --- | --- | --- | --- |
| `GET` | `/api/sessions` | `sessions.list` | `sessions:read` | — |
| `POST` | `/api/sessions` | `sessions.create` | `sessions:write` | [`SessionCreateRequestV1`](#sessioncreaterequestv1) |
| `GET` | `/api/sessions/{sessionId}` | `sessions.get` | `sessions:read` | — |
| `POST` | `/api/sessions/{sessionId}/credential-rebinds` | `sessions.credentials.rebind` | `secrets:write` | — |
| `POST` | `/api/sessions/{sessionId}/deletions` | `sessions.delete` | `sessions:delete` | — |
| `POST` | `/api/sessions/{sessionId}/forks` | `sessions.fork` | `sessions:write` | — |
| `POST` | `/api/sessions/{sessionId}/persists` | `sessions.persist` | `files:write` | — |
| `POST` | `/api/sessions/{sessionId}/stops` | `sessions.stop` | `sessions:write` | — |
| `POST` | `/api/sessions/{sessionId}/workspace/discards` | `sessions.workspace.discard` | `sessions:write` | — |

### Spans

| Method | Path | Operation | Scope | Request body |
| --- | --- | --- | --- | --- |
| `POST` | `/api/spans/listen` | `spans.listen` | `telemetry:read` | [`ObservationListenRequest`](#observationlistenrequest) |
| `POST` | `/api/spans/query` | `spans.query` | `telemetry:read` | [`ObservationQuery`](#observationquery) |
| `POST` | `/api/spans/stream` | `spans.stream` | `telemetry:read` | [`ObservationStreamRequest`](#observationstreamrequest) |

### Telemetry

| Method | Path | Operation | Scope | Request body |
| --- | --- | --- | --- | --- |
| `POST` | `/api/telemetry/exports` | `telemetry.exports.create` | `telemetry:read` | [`TelemetryExportRequest`](#telemetryexportrequest) |
| `GET` | `/api/telemetry/exports/{exportId}` | `telemetry.exports.get` | `telemetry:read` | — |
| `POST` | `/api/telemetry/exports/{exportId}/downloads` | `telemetry.exports.download` | `telemetry:read` | — |
| `POST` | `/api/telemetry/exports/{exportId}/revocations` | `telemetry.exports.revoke` | `telemetry:read` | — |
| `GET` | `/api/telemetry/gaps/{gapId}` | `telemetry.gaps.get` | `telemetry:read` | — |
| `POST` | `/api/telemetry/gaps/query` | `telemetry.gaps.query` | `telemetry:read` | [`TelemetryGapQuery`](#telemetrygapquery) |
| `POST` | `/api/telemetry/listen` | `telemetry.listen` | `telemetry:read` | [`ObservationListenRequest`](#observationlistenrequest) |
| `POST` | `/api/telemetry/otlp/v1/logs` | `telemetry.otlp.logs` | `telemetry:write` | — |
| `POST` | `/api/telemetry/otlp/v1/metrics` | `telemetry.otlp.metrics` | `telemetry:write` | — |
| `POST` | `/api/telemetry/otlp/v1/traces` | `telemetry.otlp.traces` | `telemetry:write` | — |
| `POST` | `/api/telemetry/query` | `telemetry.query` | `telemetry:read` | [`ObservationQuery`](#observationquery) |
| `POST` | `/api/telemetry/stream` | `telemetry.stream` | `telemetry:read` | [`ObservationStreamRequest`](#observationstreamrequest) |

### Traces

| Method | Path | Operation | Scope | Request body |
| --- | --- | --- | --- | --- |
| `POST` | `/api/traces/listen` | `traces.listen` | `telemetry:read` | [`ObservationListenRequest`](#observationlistenrequest) |
| `POST` | `/api/traces/query` | `traces.query` | `telemetry:read` | [`ObservationQuery`](#observationquery) |
| `POST` | `/api/traces/stream` | `traces.stream` | `telemetry:read` | [`ObservationStreamRequest`](#observationstreamrequest) |

### Uploads

| Method | Path | Operation | Scope | Request body |
| --- | --- | --- | --- | --- |
| `POST` | `/api/workspace/uploads` | `uploads.create` | `resources:write` | [`UploadCreateRequest`](#uploadcreaterequest) |
| `DELETE` | `/api/workspace/uploads/{uploadId}` | `uploads.abort` | `resources:write` | — |
| `POST` | `/api/workspace/uploads/{uploadId}/completion` | `uploads.complete` | `resources:write` | [`UploadCompleteRequest`](#uploadcompleterequest) |
| `POST` | `/api/workspace/uploads/{uploadId}/parts` | `uploads.parts` | `resources:write` | [`UploadPartsRequest`](#uploadpartsrequest) |

### Workspace resources

| Method | Path | Operation | Scope | Request body |
| --- | --- | --- | --- | --- |
| `GET` | `/api/workspace` | `workspace.get` | `workspace:read` | — |
| `GET` | `/api/workspace/limits` | `workspace.limits.list` | `workspace:read` | — |
| `GET` | `/api/workspace/limits/{limitId}` | `workspace.limits.get` | `workspace:read` | — |

## Schemas

The document's component schemas.

- [`ApiError`](#apierror)
- [`ApiKeyCreateRequest`](#apikeycreaterequest)
- [`ApprovalResponseRequest`](#approvalresponserequest)
- [`AutoTopupPolicyRequest`](#autotopuppolicyrequest)
- [`BlobDescriptor`](#blobdescriptor)
- [`DownloadGrant`](#downloadgrant)
- [`EffectiveWorkspaceLimit`](#effectiveworkspacelimit)
- [`EffectiveWorkspaceLimitPage`](#effectiveworkspacelimitpage)
- [`FileDownloadRequest`](#filedownloadrequest)
- [`InvitationCreateRequest`](#invitationcreaterequest)
- [`LiveFileDownloadRequest`](#livefiledownloadrequest)
- [`LiveFileListRequest`](#livefilelistrequest)
- [`LiveFileStatRequest`](#livefilestatrequest)
- [`Message`](#message)
- [`MessageSendRequest`](#messagesendrequest)
- [`MetricAggregationRequest`](#metricaggregationrequest)
- [`ObservationListenRequest`](#observationlistenrequest)
- [`ObservationQuery`](#observationquery)
- [`ObservationStreamRequest`](#observationstreamrequest)
- [`Operation`](#operation)
- [`OrganizationCreateRequest`](#organizationcreaterequest)
- [`PersistedFileListRequest`](#persistedfilelistrequest)
- [`PersistedFileStatRequest`](#persistedfilestatrequest)
- [`PortalSessionRequest`](#portalsessionrequest)
- [`RegisteredFileDownloadRequest`](#registeredfiledownloadrequest)
- [`RegisteredFileInput`](#registeredfileinput)
- [`RegisteredFileValue`](#registeredfilevalue)
- [`RegisteredInstructionInput`](#registeredinstructioninput)
- [`RegisteredInstructionValue`](#registeredinstructionvalue)
- [`RegisteredMcpServerInput`](#registeredmcpserverinput)
- [`RegisteredMcpServerValue`](#registeredmcpservervalue)
- [`RegisteredResource`](#registeredresource)
- [`RegisteredResourcePage`](#registeredresourcepage)
- [`RegisteredSkillInput`](#registeredskillinput)
- [`RegisteredSkillValue`](#registeredskillvalue)
- [`RegisteredToolInput`](#registeredtoolinput)
- [`RegisteredToolValue`](#registeredtoolvalue)
- [`RegistryPutResult`](#registryputresult)
- [`Run`](#run)
- [`SecretSetRequest`](#secretsetrequest)
- [`Session`](#session)
- [`SessionCreateRequestV1`](#sessioncreaterequestv1)
- [`TelemetryExportRequest`](#telemetryexportrequest)
- [`TelemetryGapQuery`](#telemetrygapquery)
- [`TopUpCheckoutRequest`](#topupcheckoutrequest)
- [`Upload`](#upload)
- [`UploadCompleteRequest`](#uploadcompleterequest)
- [`UploadCreateRequest`](#uploadcreaterequest)
- [`UploadPartsRequest`](#uploadpartsrequest)
- [`UploadPartsResponse`](#uploadpartsresponse)
- [`UsageQuery`](#usagequery)
- [`WorkspaceApiKeyValue`](#workspaceapikeyvalue)
- [`WorkspaceCreateRequest`](#workspacecreaterequest)
- [`WorkspaceDeleteRequest`](#workspacedeleterequest)

### ApiError

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `error` | `object` | yes | — |

Unknown fields are rejected.

### ApiKeyCreateRequest

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `workspaceId` | `string` | yes | — |
| `name` | `string` | yes | at least 1 character |
| `scopes` | `string[]` | yes | — |

Unknown fields are rejected.

### ApprovalResponseRequest

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `decision` | `"approve" \| "deny"` | yes | — |

Unknown fields are rejected.

### AutoTopupPolicyRequest

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `enabled` | `boolean` | yes | — |
| `thresholdUsd` | `number` | yes | — |
| `amountUsd` | `number` | yes | — |

Unknown fields are rejected.

### BlobDescriptor

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `sha256` | `string` | yes | matches `^sha256:[0-9a-f]{64}$` |
| `sizeBytes` | `integer` | yes | minimum 0; maximum 9007199254740991 |

Unknown fields are rejected.

### DownloadGrant

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `url` | `string` | yes | at least 1 character |
| `headers` | `Record<string, string>` | no | — |
| `expiresAt` | `string` | yes | matches `^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$` |
| `sizeBytes` | `integer` | yes | minimum 0; maximum 9007199254740991 |
| `authorizedBytes` | `integer` | yes | minimum 0; maximum 9007199254740991 |
| `measurementId` | `string` | yes | — |
| `sha256` | `string` | yes | matches `^sha256:[0-9a-f]{64}$` |

Unknown fields are rejected.

### EffectiveWorkspaceLimit

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `id` | `string` | yes | matches `^[a-z][a-z0-9_.-]{0,127}$` |
| `effectiveValue` | `number \| Record<string, number>` | yes | — |
| `source` | `"default" \| "workspace_override"` | yes | — |
| `adjustable` | `true` | yes | — |
| `revision` | `integer` | yes | minimum 1; maximum 9007199254740991 |
| `changedAt` | `string` | yes | matches `^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$` |

Unknown fields are rejected.

### EffectiveWorkspaceLimitPage

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `items` | `EffectiveWorkspaceLimit[]` | yes | — |
| `nextCursor` | `string` | no | matches `^cur_` |

Unknown fields are rejected.

### FileDownloadRequest

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `path` | `string` | yes | at least 1 character |
| `range` | `object` | no | — |

Unknown fields are rejected.

### InvitationCreateRequest

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `email` | `string` | yes | matches `^[^@\s]+@[^@\s]+\.[^@\s]+$` |
| `role` | `"admin" \| "member"` | yes | — |

Unknown fields are rejected.

### LiveFileDownloadRequest

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `path` | `string` | yes | at least 1 character |
| `range` | `object` | no | — |
| `wake` | `"retained" \| "never"` | no | — |
| `consistency` | `"coherent" \| "best_effort"` | no | — |
| `ifGenerationId` | `string` | no | — |

Unknown fields are rejected.

### LiveFileListRequest

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `path` | `string` | no | at least 1 character |
| `recursive` | `boolean` | no | — |
| `limit` | `integer` | no | minimum 1; maximum 9007199254740991 |
| `cursor` | `string` | no | — |
| `wake` | `"retained" \| "never"` | no | — |
| `consistency` | `"coherent" \| "best_effort"` | no | — |
| `ifGenerationId` | `string` | no | — |

Unknown fields are rejected.

### LiveFileStatRequest

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `path` | `string` | yes | at least 1 character |
| `wake` | `"retained" \| "never"` | no | — |
| `consistency` | `"coherent" \| "best_effort"` | no | — |
| `ifGenerationId` | `string` | no | — |

Unknown fields are rejected.

### Message

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `id` | `string` | yes | — |
| `sessionId` | `string` | yes | — |
| `runId` | `string` | no | — |
| `role` | `"user" \| "assistant" \| "tool"` | yes | — |
| `content` | `object \| object[]` | yes | — |
| `createdAt` | `string` | yes | matches `^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$` |

Unknown fields are rejected.

### MessageSendRequest

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `content` | `object \| object[]` | yes | at least 1 item |
| `maxSpendCents` | `integer` | no | minimum 1; maximum 9007199254740991 |

Unknown fields are rejected.

### MetricAggregationRequest

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `name` | `string` | yes | at least 1 character |
| `timeRange` | `object` | yes | — |
| `interval` | `string` | yes | at least 1 character |
| `groupBy` | `string[]` | no | at most 8 items |
| `calculations` | `object \| object[]` | yes | at least 1 item; at most 10 items |
| `where` | `unknown` | no | — |

Unknown fields are rejected.

### ObservationListenRequest

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `signals` | `"events" \| "logs" \| "spans" \| "metrics" \| "traces"[]` | no | — |
| `where` | `unknown` | no | — |
| `timeRange` | `object` | no | — |
| `order` | `object` | no | — |
| `limit` | `integer` | no | minimum 1; maximum 1000 |
| `consistency` | `"available" \| object` | no | — |

Unknown fields are rejected.

### ObservationQuery

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `signals` | `"events" \| "logs" \| "spans" \| "metrics" \| "traces"[]` | no | — |
| `where` | `unknown` | no | — |
| `timeRange` | `object` | no | — |
| `order` | `object` | no | — |
| `limit` | `integer` | no | minimum 1; maximum 1000 |
| `consistency` | `"available" \| object` | no | — |
| `cursor` | `string` | no | matches `^cur_` |

Unknown fields are rejected.

### ObservationStreamRequest

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `signals` | `"events" \| "logs" \| "spans" \| "metrics" \| "traces"[]` | no | — |
| `where` | `unknown` | no | — |
| `timeRange` | `object` | no | — |
| `order` | `object` | no | — |
| `limit` | `integer` | no | minimum 1; maximum 1000 |
| `consistency` | `"available" \| object` | no | — |
| `origin` | `object \| object \| object` | yes | — |

Unknown fields are rejected.

### Operation

`object \| object \| object \| object \| object \| object \| object \| object`.

### OrganizationCreateRequest

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `name` | `string` | yes | at least 1 character |

Unknown fields are rejected.

### PersistedFileListRequest

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `path` | `string` | no | at least 1 character |
| `recursive` | `boolean` | no | — |
| `limit` | `integer` | no | minimum 1; maximum 9007199254740991 |
| `cursor` | `string` | no | — |

Unknown fields are rejected.

### PersistedFileStatRequest

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `path` | `string` | yes | at least 1 character |

Unknown fields are rejected.

### PortalSessionRequest

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `returnUrl` | `string` | yes | matches `^https:\/\/[^/\s]+(?:\/.*)?$` |

Unknown fields are rejected.

### RegisteredFileDownloadRequest

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `range` | `object` | no | — |

Unknown fields are rejected.

### RegisteredFileInput

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `mountPath` | `string` | yes | at least 1 character |
| `content` | `object \| object` | yes | — |
| `mediaType` | `string` | yes | at least 1 character |
| `mode` | `"0644" \| "0755"` | yes | — |

Unknown fields are rejected.

### RegisteredFileValue

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `mountPath` | `string` | yes | at least 1 character |
| `content` | [`BlobDescriptor`](#blobdescriptor) | yes | — |
| `mediaType` | `string` | yes | at least 1 character |
| `mode` | `"0644" \| "0755"` | yes | — |

Unknown fields are rejected.

### RegisteredInstructionInput

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `text` | `string` | yes | — |

Unknown fields are rejected.

### RegisteredInstructionValue

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `text` | `string` | yes | — |

Unknown fields are rejected.

### RegisteredMcpServerInput

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `url` | `string` | yes | at least 1 character |
| `transport` | `"streamable_http"` | yes | — |
| `headers` | `object[]` | yes | — |

Unknown fields are rejected.

### RegisteredMcpServerValue

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `url` | `string` | yes | at least 1 character |
| `transport` | `"streamable_http"` | yes | — |
| `headers` | `object[]` | yes | — |

Unknown fields are rejected.

### RegisteredResource

`object \| object \| object \| object \| object`.

### RegisteredResourcePage

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `items` | `object \| object \| object \| object \| object[]` | yes | — |
| `nextCursor` | `string` | no | matches `^cur_` |

Unknown fields are rejected.

### RegisteredSkillInput

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `description` | `string` | yes | — |
| `bundleFormat` | `"tar.gz"` | yes | — |
| `bundle` | `object \| object` | yes | — |

Unknown fields are rejected.

### RegisteredSkillValue

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `description` | `string` | yes | — |
| `bundleFormat` | `"tar.gz"` | yes | — |
| `bundle` | [`BlobDescriptor`](#blobdescriptor) | yes | — |

Unknown fields are rejected.

### RegisteredToolInput

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `description` | `string` | yes | — |
| `inputSchema` | `Record<string, unknown>` | yes | — |
| `entry` | `string` | yes | at least 1 character |
| `bundleFormat` | `"tar.gz"` | yes | — |
| `bundle` | `object \| object` | yes | — |

Unknown fields are rejected.

### RegisteredToolValue

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `description` | `string` | yes | — |
| `inputSchema` | `Record<string, unknown>` | yes | — |
| `entry` | `string` | yes | at least 1 character |
| `bundleFormat` | `"tar.gz"` | yes | — |
| `bundle` | [`BlobDescriptor`](#blobdescriptor) | yes | — |

Unknown fields are rejected.

### RegistryPutResult

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `status` | `"created" \| "replaced" \| "unchanged"` | yes | — |
| `resource` | [`RegisteredResource`](#registeredresource) | yes | — |

Unknown fields are rejected.

### Run

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `id` | `string` | yes | — |
| `sessionId` | `string` | yes | — |
| `messageId` | `string` | yes | — |
| `status` | `"queued" \| "running" \| "succeeded" \| "failed" \| "timed_out" \| "cancelled" \| "interrupted"` | yes | — |
| `maxSpendCents` | `integer` | yes | minimum 1; maximum 9007199254740991 |
| `queuedAt` | `string` | yes | matches `^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$` |
| `startedAt` | `string` | no | matches `^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$` |
| `terminalAt` | `string` | no | matches `^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$` |
| `outputMessageIds` | `string[]` | no | — |
| `error` | `object` | no | — |
| `telemetryComplete` | `boolean` | no | — |
| `telemetryRejectionIds` | `string[]` | no | — |

Unknown fields are rejected.

### SecretSetRequest

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `value` | `string` | yes | at least 1 character |

Unknown fields are rejected.

### Session

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `id` | `string` | yes | — |
| `workspaceId` | `string` | yes | — |
| `status` | `"idle" \| "running" \| "awaiting_approval" \| "deleting"` | yes | — |
| `revision` | `integer` | yes | minimum 1; maximum 9007199254740991 |
| `persistRevision` | `integer` | yes | minimum 0; maximum 9007199254740991 |
| `createdAt` | `string` | yes | matches `^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$` |
| `updatedAt` | `string` | yes | matches `^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$` |
| `lastPersistedAt` | `string` | no | matches `^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$` |
| `continuity` | `object \| object` | yes | — |
| `lineage` | `object` | yes | — |
| `resolvedConfig` | `object` | yes | — |

Unknown fields are rejected.

### SessionCreateRequestV1

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `model` | `string` | yes | at least 1 character |
| `registered` | `object` | no | — |
| `credentials` | `object` | no | — |
| `compute` | `object` | no | — |
| `network` | `object` | no | — |
| `packages` | `object[]` | no | — |
| `approvalPolicy` | `object \| object` | no | — |
| `metadata` | `Record<string, string \| number \| boolean \| null>` | no | — |

Unknown fields are rejected.

### TelemetryExportRequest

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `query` | [`ObservationQuery`](#observationquery) | yes | — |
| `format` | `"ndjson" \| "parquet" \| "otlp_json"` | yes | — |
| `completeness` | `"require" \| "allow_gaps"` | yes | — |

Unknown fields are rejected.

### TelemetryGapQuery

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `signals` | `"events" \| "logs" \| "spans" \| "metrics" \| "traces"[]` | no | — |
| `status` | `"pending_retry" \| "open" \| "repaired"` | no | — |
| `timeRange` | `object` | no | — |
| `cursor` | `string` | no | matches `^cur_` |
| `limit` | `integer` | no | minimum 1; maximum 1000 |

Unknown fields are rejected.

### TopUpCheckoutRequest

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `amountUsd` | `number` | yes | — |
| `successUrl` | `string` | yes | matches `^https:\/\/[^/\s]+(?:\/.*)?$` |
| `cancelUrl` | `string` | yes | matches `^https:\/\/[^/\s]+(?:\/.*)?$` |

Unknown fields are rejected.

### Upload

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `id` | `string` | yes | — |
| `state` | `"pending" \| "uploading" \| "ready" \| "aborted"` | yes | — |
| `sizeBytes` | `integer` | yes | minimum 0; maximum 9007199254740991 |
| `sha256` | `string` | yes | matches `^sha256:[0-9a-f]{64}$` |
| `contentType` | `string` | yes | at least 1 character |
| `createdAt` | `string` | yes | matches `^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$` |
| `expiresAt` | `string` | yes | matches `^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$` |

Unknown fields are rejected.

### UploadCompleteRequest

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `parts` | `object[]` | yes | at least 1 item |

Unknown fields are rejected.

### UploadCreateRequest

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `sizeBytes` | `integer` | yes | minimum 0; maximum 9007199254740991 |
| `sha256` | `string` | yes | matches `^sha256:[0-9a-f]{64}$` |
| `contentType` | `string` | yes | at least 1 character |

Unknown fields are rejected.

### UploadPartsRequest

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `parts` | `object[]` | yes | at least 1 item |

Unknown fields are rejected.

### UploadPartsResponse

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `parts` | `object[]` | yes | — |

Unknown fields are rejected.

### UsageQuery

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `categories` | `"storage" \| "compute" \| "data_transfer"[]` | no | at least 1 item |
| `timeRange` | `object` | yes | — |
| `groupBy` | `"category" \| "region" \| "workspace" \| "session" \| "run" \| "operation"[]` | no | at most 6 items |
| `cursor` | `string` | no | matches `^cur_` |
| `limit` | `integer` | no | minimum 1; maximum 1000 |

Unknown fields are rejected.

### WorkspaceApiKeyValue

`string`.

### WorkspaceCreateRequest

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `organizationId` | `string` | yes | — |
| `name` | `string` | yes | at least 1 character |
| `region` | `"us-east-1" \| "us-east-2" \| "us-west-2" \| "ap-northeast-1" \| "eu-west-1"` | yes | — |

Unknown fields are rejected.

### WorkspaceDeleteRequest

| Property | Type | Required | Notes |
| --- | --- | --- | --- |
| `confirmation` | `string` | yes | — |

Unknown fields are rejected.

`unknown` marks a value the document does not describe yet. The server still
validates it — the type is simply not derivable until that field moves onto a
schema.

