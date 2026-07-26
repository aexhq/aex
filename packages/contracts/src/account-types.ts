/**
 * Client record types for the ACCOUNT and WORKSPACE-MANAGEMENT surfaces.
 *
 * Split out of `runtime-types.ts`, which keeps the PUBLIC SESSION LIFECYCLE —
 * sessions, runs, messages, files, run webhooks, whoami and runtime
 * capabilities. Nothing in that module describes an org, a plan, a key or a
 * persisted workspace resource, and nothing here is part of a caller's session
 * loop. Three families live below, each behind the section banner it arrived
 * with:
 *
 *   1. workspace secret metadata, billing (customer-facing summary, hosted
 *      checkout/portal, credit ledger) and the webhook signing secret;
 *   2. the control-plane resources served by the dashboard BFF — orgs,
 *      workspaces, API keys, members, invites;
 *   3. the route families that had NO declared client type, each DERIVED from
 *      its response schema rather than restated. That set is kept whole rather
 *      than redistributed: it includes the two WRITER-TOKEN child hops, which
 *      are session routes but not session-lifecycle types — the in-container
 *      runtime reads them with a child's writer token, never a workspace key,
 *      and no SDK caller ever holds one.
 *
 * Re-exported from the package root barrel (`index.ts`) exactly as these types
 * were when they lived in `runtime-types.ts`: this is a file boundary, not a
 * surface boundary, and every name below is still public.
 */
import type * as z from "zod/mini";
import type {
  McpServerRecordSchema,
  McpServerListResponseSchema
} from "./schemas/response-mcp-servers.js";
import type { WorkspaceWebhookDeliverySchema } from "./schemas/response-webhooks.js";
import type { WorkspaceEraseResponseSchema } from "./schemas/response-workspace.js";
import type {
  AdminBillingAccountTypeResponseSchema,
  AdminBillingPaymentMethodResponseSchema,
  AdminBillingTopupResponseSchema
} from "./schemas/response-billing.js";
import type {
  ChildFinalizeResponseSchema,
  ChildResultResponseSchema
} from "./schemas/response-sessions-internal.js";
import type { BillingAdmissionState } from "./billing-admission.js";

/**
 * Wire-level record for a workspace secret as returned by the BFF.
 *
 * Workspace secrets share the lifecycle semantic of skills/files: a
 * `Secret.value(...)` is per-session and gone at terminal, while
 * `aex.workspace.secrets.set(...)` persists a named reusable value. Use
 * `Secret.ref(name)` to bind that persisted value to a session. The
 * identity is the `name` (the handle a `Secret.ref` points at); the value
 * rotates under that stable name, bumping `version`.
 *
 * This record is METADATA ONLY — it never carries the secret value. Persisted
 * values are write-only through the public workspace-secret API.
 */
export interface SecretRecord {
  readonly id: string;
  readonly name: string;
  readonly version: number;
  readonly state: "ready";
  /** ISO-8601 with a `Z` suffix. */
  readonly createdAt?: string;
  /** ISO-8601 with a `Z` suffix. */
  readonly updatedAt?: string;
  readonly deletedAt?: string | null;
}

/**
 * One free monthly allowance, as reported by `GET /api/billing`.
 *
 * `dimension`, `unit` and `label` are OPEN strings on purpose. The quantities a
 * free allowance is denominated in, and what each one is worth, are hosted
 * billing policy; this contract states the SHAPE the server reports them in so a
 * client can render the panel without keeping a second copy of the numbers. A
 * closed union here would be exactly the duplication the prepaid model removed.
 * Contrast `admissionState`, which IS a closed union — three states, public
 * contract, not policy.
 */
export interface BillingAllowance {
  /** Server-owned dimension key, e.g. `llm_token_usd`. */
  readonly dimension: string;
  /** This period's quota, counted in {@link unit}. */
  readonly quota: number;
  readonly used: number;
  readonly remaining: number;
  /** What the quota counts, e.g. `GB`, `calls`, `USD`. */
  readonly unit: string;
  /** The customer-facing name of the dimension, e.g. `model usage`. */
  readonly label: string;
  /** ISO-8601 instant the allowance resets — the start of the next UTC month. */
  readonly resetAt: string;
  /**
   * What the remaining USD buys in tokens, on the workspace's most-used model.
   * Rides only on the USD-denominated token allowance, and only when there is
   * usage to infer a model from.
   */
  readonly approximateTokens?: {
    readonly model: string;
    readonly tokens: number;
  };
}

/** Auto-recharge settings plus the guards a top-up form has to respect. */
export interface BillingAutoTopup {
  /** Opt-in and OFF by default: a card being on file never enables recharge. */
  readonly enabled: boolean;
  /** Balance below which a recharge is triggered. */
  readonly thresholdUsd: number;
  /** Amount charged per recharge. */
  readonly amountUsd: number;
  /** Smallest accepted top-up; a smaller `amountUsd` is a `400`. */
  readonly minimumAmountUsd: number;
  /** Ceiling on successful automatic recharges per rolling 24h. */
  readonly maxPerDay: number;
}

/**
 * The saved card. `present` is authoritative and decides whether top-up and
 * auto-recharge are reachable at all; `brand`/`last4` are cosmetic and are
 * `null` when the payment provider could not be reached.
 */
export interface BillingPaymentMethod {
  readonly present: boolean;
  readonly brand: string | null;
  readonly last4: string | null;
}

/** A live block on the organization — new work is refused with `402 account_blocked`. */
export interface BillingBlock {
  /** ISO-8601 instant the block was applied. */
  readonly at: string;
  readonly reason: string;
}

/**
 * Customer-facing billing summary — `GET /api/billing` (scope `billing:read`).
 * All money fields are USD numbers.
 *
 * `planKey`, `subscriptionStatus` and `pastDueAt` are GONE with the catalog they
 * described. What replaces them is the prepaid surface: the period, the
 * card-derived {@link admissionState}, the allowance rows, the auto-recharge
 * block, the saved card, and any live block.
 *
 * This shape used to carry `[key: string]: unknown` as an "additive server
 * fields pass through" promise. That promise is precisely why `accountType` —
 * sent unconditionally — went undeclared for as long as it did, and why no
 * conformance check could notice: an index signature makes every undeclared
 * field structurally legal. It is gone; every field is declared.
 */
export interface BillingSummary {
  /** Prepaid balance (authoritative ledger sum). */
  readonly balanceUsd: number;
  /** Accrued spend for the current calendar month. */
  readonly monthSpendUsd: number;
  /** Monthly spend cap enforced on new sessions. */
  readonly spendCapUsd: number;
  /** The UTC allowance period these figures cover, `YYYY-MM`. */
  readonly period: string;
  /** What the money gates sized this workspace at. */
  readonly admissionState: BillingAdmissionState;
  /** `"internal"` marks an account exempt from the standard rate card. */
  readonly accountType: "standard" | "internal";
  /** `"active"` once a payment method is bound. Aurora-authoritative. */
  readonly paymentMethodStatus: "none" | "active";
  readonly autoTopupEnabled: boolean;
  /** `null` when the organization is not blocked. */
  readonly blocked: BillingBlock | null;
  readonly paymentMethod: BillingPaymentMethod;
  readonly autoTopup: BillingAutoTopup;
  /** One entry per free monthly allowance, in the server's canonical order. */
  readonly allowances: readonly BillingAllowance[];
}

/**
 * `POST /api/billing/topup/checkout`. One hosted Checkout does card capture,
 * address/tax collection and the credit purchase; `amountUsd` is rejected below
 * the server's minimum ({@link BillingAutoTopup.minimumAmountUsd}).
 */
export interface BillingTopupCheckoutRequest {
  readonly amountUsd: number;
  /** Optional return URL after successful hosted checkout. */
  readonly successUrl?: string;
  /** Optional return URL after checkout cancellation. */
  readonly cancelUrl?: string;
}

/**
 * `PATCH /api/billing/autotopup`. Every field is optional: an omitted field
 * keeps its stored value. Enabling requires a saved card, and `thresholdUsd`
 * must stay strictly below `amountUsd` — a threshold at or above the amount is a
 * recharge loop.
 */
export interface BillingAutoTopupRequest {
  readonly enabled?: boolean;
  readonly thresholdUsd?: number;
  readonly amountUsd?: number;
}

/** What `PATCH /api/billing/autotopup` echoes back: the stored settings. */
export interface BillingAutoTopupUpdate {
  readonly autoTopup: BillingAutoTopup;
}

/**
 * One issued monthly statement as listed by `GET /api/billing/statements`.
 *
 * Only ISSUED periods are listed: a month the generator withheld because it did
 * not reconcile is absent rather than rendered on demand.
 */
export interface BillingStatementSummary {
  /** The UTC month the statement covers, `YYYY-MM`. */
  readonly period: string;
  /** ISO-8601 instant the statement was issued. */
  readonly issuedAt: string;
  readonly openingBalanceUsd: number;
  readonly creditsPurchasedUsd: number;
  readonly usageUsd: number;
  readonly adjustmentsUsd: number;
  readonly closingBalanceUsd: number;
}

/** `GET /api/billing/statements` — the months a customer can download, newest first. */
export interface BillingStatementList {
  readonly statements: readonly BillingStatementSummary[];
}

export interface BillingPortalRequest {
  /** Optional return URL after leaving the hosted billing portal. */
  readonly returnUrl?: string;
}

/** Hosted checkout/portal session. The client should open `url`. One key. */
export interface BillingHostedSession {
  readonly url: string;
}

/**
 * One row of the ORG credit ledger as returned by `GET /api/billing/ledger`
 * (newest first). `amountUsd` is signed: top-ups are positive, run charges
 * negative.
 *
 * Every field below is selected by the handler on every row, so none is
 * optional; the nullable ones are nullable, which is a different statement. The
 * index signature this shape used to carry is gone for the reason given on
 * {@link BillingSummary}.
 */
export interface BillingLedgerEntry {
  readonly id: string;
  /** e.g. `top_up`, `session_charge`. Open server vocabulary. */
  readonly entryType: string;
  readonly amountUsd: number;
  readonly currency: string;
  /** The session this entry charges, `null` for non-run entries. */
  readonly sessionId: string | null;
  /**
   * Cost-attribution tag: the workspace the charge belongs to, `null` for
   * org-level rows such as a top-up.
   *
   * A RAW workspace id (a UUID), not the public `wsp_<hex>` form that
   * `whoami.workspaceId` and the MCP-server records carry. Same concept, two
   * renderings; compare with care.
   */
  readonly workspaceId: string | null;
  readonly description: string | null;
  readonly createdBy: string;
  /**
   * **NOT ISO-8601** — selected raw, so it arrives as the Aurora Data API's
   * `"YYYY-MM-DD HH:MM:SS"` text, no `T` and no zone. It is the last such field
   * on this surface; every other billing timestamp ({@link BillingAllowance.resetAt},
   * {@link BillingBlock.at}, {@link BillingStatementSummary.issuedAt}) is
   * constructed by its handler and IS ISO-8601 with a `Z`.
   */
  readonly createdAt: string;
}

/** Query for the billing ledger read. `limit` is clamped server-side to [1, 100] (default 25). */
export interface BillingLedgerQuery {
  readonly limit?: number;
}

/** One page of recent ledger rows (newest first). Not cursor-paged — `limit` bounds the read. */
export interface BillingLedgerPage {
  readonly entries: readonly BillingLedgerEntry[];
}

/**
 * The workspace webhook signing secret reveal — `POST /api/webhook/signing-secret`.
 * `whsec` is the Standard-Webhooks style `whsec_<base64>` string that
 * `verifyAexWebhook` accepts as `secret`. The endpoint reveals the current
 * secret, CREATING one on first use; it does not rotate (a repeat call returns
 * the same value). POST (not GET) so every reveal is a logged action.
 */
export interface WebhookSigningSecret {
  readonly whsec: string;
}

// ===========================================================================
// Control-plane resources (orgs / workspaces / API keys / members)
//
// These describe the ACCOUNT/control-plane surface served by the dashboard BFF
// (distinct from the data-plane, which self-routes on a workspace key). An org
// owns workspaces and is the billing/roles/cap boundary; a workspace stays the
// runtime tenant. Records are metadata-only. The value-bearing one-time reveals
// ({@link NewWorkspace} / {@link NewApiKey}) carry the freshly minted key
// exactly once; the SDK wraps that field in a redacted `SecretString`.
//
// These records used to be "additive-tolerant" — `[key: string]: unknown`, an
// unknown key from a newer deployment passing through rather than being
// rejected — matching the `SecretRecord` / `BillingSummary` precedent. That
// precedent is retired: an index signature makes EVERY undeclared field
// structurally legal, so no conformance check can ever report one, which is
// exactly how `BillingSummary` came to be missing two fields the server always
// sends. The declared key set is now the whole statement.
//
// UNVERIFIED, unlike the data-plane families: the control plane has no route
// table in this package, so these have no response schema and nothing checks
// them against real bytes.
// ===========================================================================

/**
 * One org the caller belongs to — the ownership / billing / roles wrapper ABOVE
 * workspaces. `role` is the caller's own membership role in this org
 * (`admin | member`); billing and the per-org workspace cap live at this level.
 */
export interface OrgRecord {
  readonly id: string;
  readonly name: string;
  /** Globally-unique org slug (`/org/<slug>`); omitted by older deployments. */
  readonly slug?: string;
  /** Plan key that governs billing + the per-org workspace cap (e.g. `free`). */
  readonly planKey?: string;
  /** The caller's role in this org: `admin` or `member`. */
  readonly role?: string;
  readonly createdAt?: string;
}

/** Request body for {@link createOrg} — a display name; the server assigns id/slug. */
export interface CreateOrgRequest {
  readonly name: string;
}

/**
 * A workspace as seen from the CONTROL plane (management view): its id, name,
 * and owning org. Distinct from the data-plane view — this never carries the
 * workspace's files/skills/secrets, only the row a dashboard/CLI lists.
 */
export interface WorkspaceRecord {
  readonly id: string;
  readonly name: string;
  /** Globally-unique workspace slug (`/workspace/<slug>`); omitted by older deployments. */
  readonly slug?: string;
  /** The org that owns this workspace. */
  readonly orgId: string;
  readonly createdAt?: string;
}

/** Request body for {@link createWorkspace}. Free tier caps at 3 workspaces per org. */
export interface CreateWorkspaceRequest {
  /** The org to create the workspace under. */
  readonly orgId: string;
  readonly name: string;
}

/**
 * One-time reveal returned by {@link createWorkspace}: the new workspace's id
 * plus its FIRST workspace-scoped, data-plane API key. The key is shown exactly
 * once at creation — the creating (account) principal has no other data-plane
 * access to it, though the owning user can see/delete it in the dashboard
 * (orphan recovery). The SDK wraps `apiKey` in a redacted `SecretString`.
 */
export interface NewWorkspace {
  readonly workspaceId: string;
  /** The workspace-scoped API key (`aex_<plane>_…`), revealed ONCE. */
  readonly apiKey: string;
  /** Globally-unique workspace slug, when the server assigns one. */
  readonly slug?: string;
  /** The org that owns the new workspace. */
  readonly orgId?: string;
}

/**
 * Metadata for one API key (data-plane workspace key OR account PAT). NEVER
 * carries the secret value — the value is write-only and revealed only once via
 * {@link NewApiKey}. `kind` distinguishes a `workspace` key from an `account`
 * PAT; `workspaceId` is present only for workspace keys.
 */
export interface ApiKeyRecord {
  readonly id: string;
  readonly name?: string;
  /** `workspace` (data-plane) or `account` (control-plane PAT). */
  readonly kind?: string;
  /** Present for workspace keys; absent for account PATs. */
  readonly workspaceId?: string;
  readonly scopes?: readonly string[];
  readonly createdAt?: string;
  readonly lastUsedAt?: string | null;
  readonly revokedAt?: string | null;
}

/**
 * Request body for {@link createApiKey}. Mint EITHER a workspace-scoped
 * data-plane key (pass `workspaceId`) or an account PAT (`account: true`) — the
 * two are mutually exclusive. Anti-escalation: an account PAT can mint workspace
 * keys but not another PAT (enforced server-side).
 */
export interface CreateApiKeyRequest {
  /** Mint a WORKSPACE-scoped data-plane key for this workspace. */
  readonly workspaceId?: string;
  /** Mint an ACCOUNT PAT (control-plane) instead. Mutually exclusive with `workspaceId`. */
  readonly account?: boolean;
  /** Optional human label for the key. */
  readonly name?: string;
  /** Optional scope restriction; defaults server-side. */
  readonly scopes?: readonly string[];
}

/**
 * One-time reveal returned by {@link createApiKey}: the key id plus the freshly
 * minted secret value, shown exactly once. The SDK wraps `apiKey` in a redacted
 * `SecretString`.
 */
export interface NewApiKey {
  readonly id: string;
  /** The freshly minted key value, revealed ONCE. */
  readonly apiKey: string;
  readonly name?: string;
  readonly kind?: string;
  readonly workspaceId?: string;
  readonly scopes?: readonly string[];
}

/** One member of an org (from {@link listOrgMembers}). Never carries credentials. */
export interface OrgMemberRecord {
  /** The member's stable account (app-user) id. */
  readonly appUserId: string;
  readonly email?: string;
  /** `admin` or `member`. */
  readonly role: string;
  /** `active` or `pending` (an unaccepted invite). */
  readonly status?: string;
  readonly createdAt?: string;
}

/** Request body for {@link createOrgInvite} — invite an email at a role. */
export interface CreateOrgInviteRequest {
  readonly email: string;
  /** `admin` or `member`; defaults server-side to `member`. */
  readonly role?: string;
}

/**
 * A pending team invite created by {@link createOrgInvite}. Metadata only — the
 * invite token itself is delivered out-of-band (email), never returned here.
 */
export interface OrgInvite {
  readonly id: string;
  readonly orgId: string;
  readonly email: string;
  readonly role: string;
  /** `pending` until accepted. */
  readonly status?: string;
  readonly expiresAt?: string;
  readonly createdAt?: string;
}

// ===========================================================================
// Route families that had NO declared client type
//
// Six groups of public data-plane routes were reachable only by hand-rolling a
// request and casting the result: every `mcp-servers` route, the workspace
// webhook-delivery list, the three operator billing overrides, the data-plane
// workspace erase, and the two writer-token child hops. Each is DERIVED from its
// response schema (`z.infer`) rather than restated, so the type and the bytes
// the C4 harness validates come from one declaration.
// ===========================================================================

/**
 * One persisted workspace MCP server — `POST/GET /api/mcp-servers`,
 * `GET/DELETE /api/mcp-servers/{id}`.
 *
 * `headerShape` lists header NAMES ONLY. The values live in the secret store and
 * are write-only through this API; a `headers` or `authorization` key appearing
 * on a read is a leak, and the response schema fails the suite if one does.
 *
 * `workspaceId` is the PUBLIC `wsp_<hex>` form (the handler runs it through
 * `publicWorkspaceId`), matching `whoami.workspaceId` — and NOT matching
 * {@link BillingLedgerEntry.workspaceId} or {@link WorkspaceEraseResult}, which
 * are raw ids. `createdAt` / `updatedAt` are ISO-8601 with a `Z`.
 */
export type McpServerRecord = z.infer<typeof McpServerRecordSchema>;

/** `GET /api/mcp-servers` — every workspace MCP server, newest first. Not paged. */
export type McpServerList = z.infer<typeof McpServerListResponseSchema>;

/**
 * One row of `GET /api/webhook/deliveries` — the WORKSPACE view of the delivery
 * ledger, which is {@link SessionWebhookDelivery} plus the `sessionId` and
 * `callbackUrl` it belongs to. Server-capped at the 100 most recent: no `limit`,
 * no cursor.
 */
export type WorkspaceWebhookDelivery = z.infer<typeof WorkspaceWebhookDeliverySchema>;

/**
 * `DELETE /api/workspaces/{workspaceId}` on the DATA plane — the owner's GDPR
 * hard-erase, answering 200 with counters rather than 204. Idempotent: erasing
 * an absent workspace answers 200 with every counter at zero, indistinguishable
 * from erasing an empty one.
 *
 * NOT the same route as {@link deleteWorkspace}, which is the CONTROL plane's
 * `DELETE /api/workspaces/{id}` and returns no body. Same method, same path
 * pattern, different plane and different response.
 *
 * `workspaceId` here is the RAW id, not the public `wsp_<hex>` form.
 */
export type WorkspaceEraseResult = z.infer<typeof WorkspaceEraseResponseSchema>;

/** `POST /api/admin/billing/topup` — operator credit grant. */
export type AdminBillingTopupResult = z.infer<typeof AdminBillingTopupResponseSchema>;

/** `POST /api/admin/billing/payment-method` — operator payment-method override. */
export type AdminBillingPaymentMethodResult = z.infer<
  typeof AdminBillingPaymentMethodResponseSchema
>;

/** `POST /api/admin/billing/account-type` — operator account-type override. */
export type AdminBillingAccountTypeResult = z.infer<typeof AdminBillingAccountTypeResponseSchema>;

/**
 * `GET /api/sessions/{id}/result` — a subagent child's result, read by the
 * in-container runtime with the child's WRITER TOKEN, not a workspace API key.
 * Either still in flight (`settling` / `running` / `queued`) or `finished` with
 * an outcome.
 *
 * Its files key the workspace-relative path as **`path`**, where the public
 * `/files` routes call the same thing `filename` ({@link SessionFile}). Two
 * server projections of one concept differing in one key name.
 */
export type ChildResult = z.infer<typeof ChildResultResponseSchema>;

/**
 * `POST /api/sessions/{id}/finalize` — the writer-token child settle hop. Note
 * the absence of a `session` envelope: this route is not shaped like the public
 * session routes.
 */
export type ChildFinalizeResult = z.infer<typeof ChildFinalizeResponseSchema>;
