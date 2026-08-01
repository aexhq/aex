---
# GENERATED FILE - do not edit.
# Written by `scripts/docs/generate-all.mjs`.
# Edit the source and run `bun run docs:generate`. A hand edit here is
# reverted by the next `bun run lint`, which regenerates via `prelint`.
title: "SDK"
description: "Generated TypeScript SDK reference."
---

# SDK

Generated from `packages/sdk/src/index.ts` with TypeDoc.

## Classes

### `AccountClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `get` (Method)

### `Aex`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `account` (Property)
- `apiKeys` (Property)
- `billing` (Property)
- `events` (Property)
- `logs` (Property)
- `metrics` (Property)
- `operations` (Property)
- `organizations` (Property)
- `sessions` (Property)
- `spans` (Property)
- `telemetry` (Property)
- `traces` (Property)
- `workspace` (Property)
- `workspaces` (Property)

### `AexApiError`

Thrown by SDK and CLI operations when the hosted aex API returns a non-2xx
response. Carries the HTTP status, the redacted parsed body, the server's
STABLE AexApiErrorCode (when present), and a `requestId` for support.
Construct via import("./error-factory.js").apiErrorFromResponse — the
single wire→exception mapping — which dispatches to a typed subclass
(AexAuthError / AexIdempotencyConflictError /
AexNotFoundError / AexRateLimitError).

Members:

- `constructor` (Constructor)
- `apiCode` (Property)
- `body` (Property)
- `requestId` (Property)
- `status` (Property)

### `AexAuthError`

401/403 auth failure (token invalid/revoked/expired, forbidden, insufficient scope).

Members:

- `constructor` (Constructor)
- `requiredScope` (Property)

### `AexError`

Members:

- `constructor` (Constructor)
- `code` (Property)
- `details` (Property)

### `AexIdempotencyConflictError`

409 — the idempotency key was reused with a different request body.

Members:

- `constructor` (Constructor)

### `AexNetworkError`

Thrown when a BFF-bound request fails BEFORE any HTTP response exists — DNS
failure, connection refused, TLS error, socket reset. Wraps the raw fetch
rejection (whose undici form is a bare `TypeError: fetch failed` with the
useful code hidden on `cause.code`) into a message that names the request
and the transport failure, e.g.
`POST api.aex.dev/api/sessions failed: ECONNREFUSED (connect ECONNREFUSED 127.0.0.1:443)`.
The original rejection is preserved on `cause`.

Members:

- `constructor` (Constructor)
- `attempts` (Property)
- `causeCode` (Property)
- `elapsedMs` (Property)
- `host` (Property)
- `method` (Property)
- `path` (Property)

### `AexNotFoundError`

404 — the requested resource was not found.

Members:

- `constructor` (Constructor)

### `ApiKeysClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `create` (Method)
- `list` (Method)
- `revoke` (Method)

### `BillingAutoTopupClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `get` (Method)
- `replace` (Method)

### `BillingBalanceClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `get` (Method)

### `BillingClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `autoTopup` (Property)
- `balance` (Property)
- `statements` (Property)
- `usage` (Property)
- `portalSession` (Method)
- `topUpCheckout` (Method)

### `BillingStatementsClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `download` (Method)
- `get` (Method)
- `list` (Method)

### `BillingUsageClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `query` (Method)
- `queryOrganization` (Method)

### `CredentialValidationError`

Members:

- `constructor` (Constructor)

### `LiveSessionFilesClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `download` (Method)
- `list` (Method)
- `stat` (Method)

### `MetricsClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `aggregate` (Method)

### `ObservationIterator`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `coverage` (Property)
- `[asyncIterator]` (Method)

### `ObservationSignalClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `iterate` (Method)
- `listen` (Method)
- `query` (Method)
- `stream` (Method)

### `OperationFailedError`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `operation` (Property)
- `operationId` (Property)

### `OperationHandle`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `id` (Accessor)
- `kind` (Accessor)
- `record` (Accessor)
- `status` (Accessor)
- `get` (Method)
- `result` (Method)
- `wait` (Method)

### `OperationsClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `cancel` (Method)
- `get` (Method)
- `list` (Method)
- `open` (Method)

### `OrganizationInvitationsClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `create` (Method)

### `OrganizationMembershipsClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `list` (Method)

### `OrganizationsClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `invitations` (Property)
- `memberships` (Property)
- `create` (Method)
- `get` (Method)
- `list` (Method)

### `PersistedSessionFilesClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `download` (Method)
- `list` (Method)
- `stat` (Method)

### `RegisteredFileClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `download` (Method)

### `RegisteredResourceClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `delete` (Method)
- `get` (Method)
- `list` (Method)
- `set` (Method)

### `RunFailedError`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `run` (Property)

### `RunHandle`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `id` (Accessor)
- `record` (Accessor)
- `status` (Accessor)
- `get` (Method)
- `result` (Method)
- `wait` (Method)

### `ScopedTelemetryClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `exports` (Property)
- `gaps` (Property)
- `export` (Method)

### `SessionApprovalsClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `get` (Method)
- `list` (Method)
- `respond` (Method)

### `SessionCredentialsClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `rebind` (Method)

### `SessionFilesClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `live` (Property)
- `persisted` (Property)

### `SessionHandle`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `approvals` (Property)
- `credentials` (Property)
- `events` (Property)
- `files` (Property)
- `logs` (Property)
- `messages` (Property)
- `metrics` (Property)
- `spans` (Property)
- `telemetry` (Property)
- `traces` (Property)
- `workspace` (Property)
- `id` (Accessor)
- `record` (Accessor)
- `admitOperation` (Method)
- `delete` (Method)
- `fork` (Method)
- `persist` (Method)
- `stop` (Method)

### `SessionMessagesClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `send` (Method)

### `SessionsClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `create` (Method)
- `get` (Method)
- `list` (Method)
- `open` (Method)

### `SessionWorkspaceClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `discard` (Method)

### `TelemetryClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `otlp` (Property)

### `TelemetryExportsClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `download` (Method)
- `get` (Method)
- `revoke` (Method)

### `TelemetryGapsClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `get` (Method)
- `query` (Method)

### `TelemetryOtlpClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `logs` (Method)
- `metrics` (Method)
- `traces` (Method)

### `TelemetryStreamBackpressureError`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `cursor` (Property)

### `TelemetryStreamProtocolError`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)

### `TelemetryStreamRotationError`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `cursor` (Property)

### `TracesClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `get` (Method)

### `WorkspaceClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `files` (Property)
- `instructions` (Property)
- `limits` (Property)
- `mcpServers` (Property)
- `secrets` (Property)
- `skills` (Property)
- `tools` (Property)
- `uploads` (Property)
- `get` (Method)

### `WorkspaceLimitsClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `get` (Method)
- `list` (Method)

### `WorkspacesClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `create` (Method)
- `delete` (Method)
- `get` (Method)
- `list` (Method)

### `WorkspaceSecretsClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `delete` (Method)
- `get` (Method)
- `list` (Method)
- `revoke` (Method)
- `set` (Method)

### `WorkspaceUploadsClient`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

Members:

- `constructor` (Constructor)
- `abort` (Method)
- `complete` (Method)
- `create` (Method)
- `parts` (Method)

## Interfaces

### `AccountGetQuery`

Members:

- `organizationId` (Property)

### `AexOptions`

Members:

- `apiKey` (Property)
- `baseUrl` (Property)
- `bootstrapBaseUrl` (Property)
- `debug` (Property)
- `fetch` (Property)
- `retry` (Property)

### `ApiKeyListQuery`

Members:

- `cursor` (Property)
- `limit` (Property)
- `workspaceId` (Property)

### `ApprovalListQuery`

Members:

- `cursor` (Property)
- `limit` (Property)

### `AutoTopupReplaceOptions`

Members:

- `ifRevision` (Property)

### `BillingBalanceQuery`

Members:

- `organizationId` (Property)

### `CoordinatedDownloadGrant`

Members:

- `count` (Property)
- `grant` (Property)
- `index` (Property)
- `range` (Property)

### `CoordinateDownloadGrantsInput`

Members:

- `mint` (Property)
- `range` (Property)
- `sha256` (Property)
- `sizeBytes` (Property)

### `CredentialRebindRequest`

Members:

- `secrets` (Property)

### `DownloadRange`

Members:

- `endExclusive` (Property)
- `start` (Property)

### `IdempotencyOptions`

Members:

- `idempotencyKey` (Property)

### `MessageSendAccepted`

Members:

- `message` (Property)
- `run` (Property)

### `ObservationQuery`

Members:

- `consistency` (Property)
- `cursor` (Property)
- `limit` (Property)
- `order` (Property)
- `signals` (Property)
- `timeRange` (Property)
- `where` (Property)

### `OperationAdmissionOptions`

Members:

- `operationId` (Property)

### `OperationListQuery`

Members:

- `cursor` (Property)
- `kind` (Property)
- `limit` (Property)
- `sessionId` (Property)
- `status` (Property)

### `OrganizationListQuery`

Members:

- `cursor` (Property)
- `limit` (Property)

### `OrganizationUsageResult`

Members:

- `frontiers` (Property)
- `items` (Property)

### `OtlpAdmissionOptions`

Members:

- `batchId` (Property)
- `contentType` (Property)

### `OtlpAdmissionReceipt`

Members:

- `batchId` (Property)
- `receiptId` (Property)
- `response` (Property)

### `RegisteredResourceInputByKind`

Members:

- `file` (Property)
- `instruction` (Property)
- `mcp_server` (Property)
- `skill` (Property)
- `tool` (Property)

### `RevisionMutationOptions`

### `RevisionOperationAdmissionOptions`

Members:

- `ifRevision` (Property)

### `RevisionOptions`

Members:

- `ifRevision` (Property)

### `SessionDeleteRequest`

Members:

- `cascade` (Property)

### `SessionForkRequest`

Members:

- `credentials` (Property)
- `files` (Property)

### `SessionListQuery`

Members:

- `cursor` (Property)
- `limit` (Property)
- `status` (Property)

### `SessionPersistRequest`

Members:

- `exclude` (Property)
- `include` (Property)

### `TelemetryGapPage`

Members:

- `coverage` (Property)
- `items` (Property)
- `nextCursor` (Property)

### `WaitOptions`

Members:

- `pollIntervalMs` (Property)
- `signal` (Property)
- `timeoutMs` (Property)

### `WorkspaceDeleteOptions`

Members:

- `confirmation` (Property)

### `WorkspaceDiscardRequest`

Members:

- `ifGenerationId` (Property)

### `WorkspaceListQuery`

Members:

- `cursor` (Property)
- `limit` (Property)
- `organizationId` (Property)

## Functions

### `apiErrorFromResponse`

`apiErrorFromResponse(input)`

### `coordinateDownloadGrants`

Mints one grant at a time for a provider-bounded full or partial download.
It validates object identity and authorization metadata before yielding the
bearer URL, so callers can fetch and discard each grant before minting the
next one.

`coordinateDownloadGrants(input)`

### `isAuthError`

True for a 401/403 authentication/authorization failure.

`isAuthError(err)`

### `isIdempotencyConflict`

True for a 409 idempotency-key reuse conflict.

`isIdempotencyConflict(err)`

### `isNotFound`

True for a 404 not-found error.

`isNotFound(err)`

### `planDownloadRanges`

Public v1 SDK surface.

Execution is explicitly session-oriented. Long-running mutations return
durable operation handles; run and operation waits are client-side GET
polling conveniences, not separate REST resources.

`planDownloadRanges(sizeBytes, selected)`

## Variables

### `MAX_SINGLE_GET_BYTES`

AWS S3's provider-hard maximum selected byte range for one GetObject.

