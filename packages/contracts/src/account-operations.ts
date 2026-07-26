/**
 * Client operations for the ACCOUNT and WORKSPACE-MANAGEMENT surfaces: billing,
 * the workspace secret store, and the control-plane orgs / workspaces / API
 * keys / members.
 *
 * Split out of `operations.ts`, which keeps the session-lifecycle transport
 * (create / message / stream / files / archive / workspace resources). The seam
 * is the subject: nothing here touches a session, and every function below is
 * re-exported from `operations.ts`, so the `operations` namespace on
 * `@aexhq/contracts/internal` is exactly what it was.
 *
 * The same rule as its parent applies: every function takes an `HttpClient` so
 * callers own auth + fetch injection, and workspace identity is derived
 * server-side from the API key on the data plane. Control-plane calls pass org /
 * workspace / key ids EXPLICITLY, because a control-plane principal spans many.
 */
import type { HttpClient } from "./http.js";
import { SessionStateError } from "./sdk-errors.js";
import { isRecord } from "./value-guards.js";
import { configError, resolveIdempotencyKey, type IdempotencyOptions } from "./operation-core.js";
import type {
  ApiKeyRecord,
  BillingAutoTopupRequest,
  BillingAutoTopupUpdate,
  BillingHostedSession,
  BillingLedgerPage,
  BillingLedgerQuery,
  BillingPortalRequest,
  BillingSummary,
  BillingTopupCheckoutRequest,
  CreateApiKeyRequest,
  CreateOrgInviteRequest,
  CreateOrgRequest,
  CreateWorkspaceRequest,
  NewApiKey,
  NewWorkspace,
  OrgInvite,
  OrgMemberRecord,
  OrgRecord,
  SecretRecord,
  WebhookSigningSecret,
  WorkspaceRecord
} from "./account-types.js";

/**
 * Read the workspace billing summary (`GET /api/billing`, scope `billing:read`):
 * prepaid balance, current-month spend, spend cap, the free monthly allowances,
 * auto-recharge settings and the saved card.
 */
export async function getBilling(http: HttpClient): Promise<BillingSummary> {
  return http.request<BillingSummary>("/api/billing");
}

function rejectBodyIdempotencyKey(request: unknown): void {
  if (isRecord(request) && Object.prototype.hasOwnProperty.call(request, "idempotencyKey")) {
    throw configError(
      "idempotencyKey",
      "billing idempotencyKey belongs in the second options argument, not the request body"
    );
  }
}

function resolveBillingIdempotencyKey(request: unknown, options?: IdempotencyOptions): string {
  rejectBodyIdempotencyKey(request);
  const idempotencyKey = resolveIdempotencyKey(options?.idempotencyKey);
  return idempotencyKey;
}

/**
 * Buy prepaid credit (`POST /api/billing/topup/checkout`). Returns only the
 * hosted URL; the balance moves after the charge settles, not when this
 * resolves. The same flow captures the card on first use.
 */
export async function createBillingTopupCheckout(
  http: HttpClient,
  request: BillingTopupCheckoutRequest,
  options?: IdempotencyOptions
): Promise<BillingHostedSession> {
  const idempotencyKey = resolveBillingIdempotencyKey(request, options);
  return http.request<BillingHostedSession>("/api/billing/topup/checkout", {
    method: "POST",
    headers: { "Idempotency-Key": idempotencyKey },
    body: JSON.stringify(request)
  });
}

/**
 * Update auto-recharge (`PATCH /api/billing/autotopup`). Omitted fields keep
 * their stored value; the response echoes the stored settings.
 *
 * No idempotency key: this is a whole-state PATCH, so a replay writes the same
 * row. The body guard stays — an `idempotencyKey` in the body was never a
 * request field and silently ignoring it would look like it worked.
 */
export async function updateBillingAutoTopup(
  http: HttpClient,
  request: BillingAutoTopupRequest
): Promise<BillingAutoTopupUpdate> {
  rejectBodyIdempotencyKey(request);
  return http.request<BillingAutoTopupUpdate>("/api/billing/autotopup", {
    method: "PATCH",
    body: JSON.stringify(request)
  });
}

/**
 * Create a hosted billing-portal session for the workspace customer.
 * Returns only the hosted URL.
 */
export async function createBillingPortal(
  http: HttpClient,
  request: BillingPortalRequest = {},
  options?: IdempotencyOptions
): Promise<BillingHostedSession> {
  const idempotencyKey = resolveBillingIdempotencyKey(request, options);
  return http.request<BillingHostedSession>("/api/billing/portal", {
    method: "POST",
    headers: { "Idempotency-Key": idempotencyKey },
    body: JSON.stringify(request)
  });
}

/**
 * Read recent workspace credit-ledger rows (`GET /api/billing/ledger`, scope
 * `billing:read`), newest first. `limit` is clamped server-side to [1, 100]
 * (default 25); the read is not cursor-paged.
 */
export async function getBillingLedger(
  http: HttpClient,
  query?: BillingLedgerQuery
): Promise<BillingLedgerPage> {
  const params: Record<string, string> = {};
  if (query?.limit !== undefined) params.limit = String(query.limit);
  return http.request<BillingLedgerPage>("/api/billing/ledger", {}, params);
}

/**
 * Reveal the workspace webhook signing secret (`POST /api/webhook/signing-secret`),
 * CREATING one on first use. Repeat calls return the same `whsec_<base64>` value —
 * the hosted API does not rotate it. POST (not GET) so a reveal is a logged action.
 * Pass the returned `whsec` to `verifyAexWebhook` as `secret`.
 */
export async function getWebhookSigningSecret(http: HttpClient): Promise<WebhookSigningSecret> {
  return http.request<WebhookSigningSecret>("/api/webhook/signing-secret", { method: "POST" });
}

// ===========================================================================
// Workspace secret operations
//
// Value-bearing requests (create/rotate) carry the value in the JSON BODY,
// never the URL/query, so it never lands in logs or the request line. Reads
// return metadata only; persisted secret values are write-only through this API.
// ===========================================================================

/** Create a named workspace secret. The value travels in the body. */
export async function createSecret(
  http: HttpClient,
  args: { readonly name: string; readonly value: string }
): Promise<SecretRecord> {
  const result = await http.request<{ readonly secret: SecretRecord }>("/api/secrets", {
    method: "POST",
    body: JSON.stringify({ name: args.name, value: args.value })
  });
  return unwrapSecret(result);
}

export async function listSecrets(http: HttpClient): Promise<readonly SecretRecord[]> {
  const result = await http.request<unknown>("/api/secrets");
  if (!isRecord(result) || !Array.isArray(result.secrets)) {
    throw new SessionStateError("workspace secrets response must contain a secrets array");
  }
  return result.secrets as readonly SecretRecord[];
}

/** Metadata for one workspace secret by name. Never returns the value. */
export async function getSecret(http: HttpClient, name: string): Promise<SecretRecord> {
  const result = await http.request<{ readonly secret: SecretRecord }>(
    `/api/secrets/${encodeURIComponent(name)}`
  );
  return unwrapSecret(result);
}

/** Replace the value of an existing workspace secret; bumps its version. */
export async function rotateSecret(
  http: HttpClient,
  args: { readonly name: string; readonly value: string }
): Promise<SecretRecord> {
  const result = await http.request<{ readonly secret: SecretRecord }>(
    `/api/secrets/${encodeURIComponent(args.name)}/rotate`,
    { method: "POST", body: JSON.stringify({ value: args.value }) }
  );
  return unwrapSecret(result);
}

export async function deleteSecret(http: HttpClient, name: string): Promise<void> {
  await http.request<unknown>(`/api/secrets/${encodeURIComponent(name)}`, {
    method: "DELETE"
  });
}

function unwrapSecret(result: { readonly secret: SecretRecord }): SecretRecord {
  if (!isRecord(result) || !isRecord(result.secret)) {
    throw new SessionStateError("workspace secret response must contain a secret object");
  }
  return result.secret as unknown as SecretRecord;
}

// ===========================================================================
// Control-plane operations (orgs / workspaces / API keys / members)
//
// These target the ACCOUNT/control-plane surface on the dashboard BFF — reached
// with an account PAT / device session, NOT a data-plane workspace key. They
// mirror the publish/list/get/delete generic and the value-bearing one-time
// reveal shapes above (create returns the key exactly once). Endpoints:
//   orgs:       POST/GET  /api/orgs, GET /api/orgs/:orgId/members,
//               POST /api/orgs/:orgId/invites
//   workspaces: POST/GET  /api/workspaces, DELETE /api/workspaces/:id
//   keys:       POST/GET  /api/keys, DELETE /api/keys/:id
// Workspace/org identity is passed EXPLICITLY here (unlike the data plane, which
// derives the workspace from the key) because a control-plane principal spans
// multiple orgs and workspaces.
// ===========================================================================

/** Create an org (the caller becomes its admin). `POST /api/orgs`. */
export async function createOrg(http: HttpClient, request: CreateOrgRequest): Promise<OrgRecord> {
  const result = await http.request<{ readonly org: OrgRecord }>("/api/orgs", {
    method: "POST",
    body: JSON.stringify(request)
  });
  return unwrapControlRecord(result, "org", "org");
}

/** List the orgs the caller belongs to. `GET /api/orgs`. */
export async function listOrgs(http: HttpClient): Promise<readonly OrgRecord[]> {
  return listControlRecords<OrgRecord>(http, "/api/orgs", "orgs");
}

/** List an org's members (and pending invites, as `status: "pending"`). `GET /api/orgs/:orgId/members`. */
export async function listOrgMembers(http: HttpClient, orgId: string): Promise<readonly OrgMemberRecord[]> {
  requireControlId(orgId, "orgId", "listOrgMembers");
  return listControlRecords<OrgMemberRecord>(
    http,
    `/api/orgs/${encodeURIComponent(orgId)}/members`,
    "members"
  );
}

/** Invite an email to an org at a role. `POST /api/orgs/:orgId/invites`. */
export async function createOrgInvite(
  http: HttpClient,
  orgId: string,
  request: CreateOrgInviteRequest
): Promise<OrgInvite> {
  requireControlId(orgId, "orgId", "createOrgInvite");
  const result = await http.request<{ readonly invite: OrgInvite }>(
    `/api/orgs/${encodeURIComponent(orgId)}/invites`,
    { method: "POST", body: JSON.stringify(request) }
  );
  return unwrapControlRecord(result, "invite", "org invite");
}

/**
 * Create a workspace under an org and mint its FIRST workspace-scoped API key,
 * returned once as {@link NewWorkspace}. `POST /api/workspaces`. The free tier
 * caps at 3 workspaces per org (the server surfaces a 409 when exceeded).
 */
export async function createWorkspace(
  http: HttpClient,
  request: CreateWorkspaceRequest
): Promise<NewWorkspace> {
  const result = await http.request<{ readonly workspace: NewWorkspace }>("/api/workspaces", {
    method: "POST",
    body: JSON.stringify(request)
  });
  const workspace = unwrapControlRecord<NewWorkspace>(result, "workspace", "new workspace");
  if (typeof workspace.workspaceId !== "string" || workspace.workspaceId.length === 0) {
    throw new SessionStateError("createWorkspace response is missing workspaceId");
  }
  if (typeof workspace.apiKey !== "string" || workspace.apiKey.length === 0) {
    throw new SessionStateError("createWorkspace response is missing the one-time apiKey");
  }
  return workspace;
}

/** List the workspaces the caller can manage across their orgs. `GET /api/workspaces`. */
export async function listWorkspaces(http: HttpClient): Promise<readonly WorkspaceRecord[]> {
  return listControlRecords<WorkspaceRecord>(http, "/api/workspaces", "workspaces");
}

/** Delete a workspace by id. `DELETE /api/workspaces/:id`. Idempotent. */
export async function deleteWorkspace(http: HttpClient, workspaceId: string): Promise<void> {
  requireControlId(workspaceId, "workspaceId", "deleteWorkspace");
  await http.request<void>(`/api/workspaces/${encodeURIComponent(workspaceId)}`, { method: "DELETE" });
}

/**
 * Mint an API key, returned once as {@link NewApiKey}. `POST /api/keys`. Pass
 * `workspaceId` for a data-plane workspace key, or `account: true` for an
 * account PAT (control-plane). A PAT cannot mint another PAT (anti-escalation,
 * enforced server-side).
 */
export async function createApiKey(http: HttpClient, request: CreateApiKeyRequest = {}): Promise<NewApiKey> {
  if (request.account === true && request.workspaceId !== undefined) {
    throw configError("account", "createApiKey: pass either workspaceId or account:true, not both");
  }
  const result = await http.request<{ readonly key: NewApiKey }>("/api/keys", {
    method: "POST",
    body: JSON.stringify(request)
  });
  const key = unwrapControlRecord<NewApiKey>(result, "key", "new api key");
  if (typeof key.id !== "string" || key.id.length === 0) {
    throw new SessionStateError("createApiKey response is missing the key id");
  }
  if (typeof key.apiKey !== "string" || key.apiKey.length === 0) {
    throw new SessionStateError("createApiKey response is missing the one-time apiKey");
  }
  return key;
}

/** List API keys (metadata only; never values). `GET /api/keys`. */
export async function listApiKeys(http: HttpClient): Promise<readonly ApiKeyRecord[]> {
  return listControlRecords<ApiKeyRecord>(http, "/api/keys", "keys");
}

/** Revoke/delete an API key by id. `DELETE /api/keys/:id`. Idempotent. */
export async function deleteApiKey(http: HttpClient, keyId: string): Promise<void> {
  requireControlId(keyId, "keyId", "deleteApiKey");
  await http.request<void>(`/api/keys/${encodeURIComponent(keyId)}`, { method: "DELETE" });
}

function requireControlId(value: string, field: string, context: string): void {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw configError(field, `${context}: ${field} must be a non-empty string`);
  }
}

/** Unwrap a `{ <key>: T }` single-record control-plane envelope, validating shape. */
function unwrapControlRecord<T>(result: unknown, key: string, label: string): T {
  if (!isRecord(result) || !isRecord(result[key])) {
    throw new SessionStateError(`${label} response must contain a ${key} object`);
  }
  return result[key] as unknown as T;
}

/** Unwrap a `{ <key>: T[] }` list control-plane envelope, validating shape. */
async function listControlRecords<T>(http: HttpClient, path: string, key: string): Promise<readonly T[]> {
  const result = await http.request<unknown>(path);
  if (!isRecord(result) || !Array.isArray(result[key])) {
    throw new SessionStateError(`${path} response must contain a ${key} array`);
  }
  return result[key] as readonly T[];
}
