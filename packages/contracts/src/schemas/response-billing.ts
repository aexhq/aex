/**
 * Response schemas for the `billing.*` and `adminBilling.*` families.
 *
 * Three places the server and the declared types disagree, all resolved in
 * favour of the server (a schema that fails every real response is not a gate):
 *
 * 1. `BillingSummary` does not declare `accountType` or `pastDueAt`; the server
 *    sends both, unconditionally.
 * 2. `BillingLedgerEntry` does not declare `workspaceId`; the server sends it on
 *    every row (the WS2 cost-attribution tag), `null` for org-level rows.
 * 3. Both interfaces USED to carry `[key: string]: unknown` — an explicit
 *    "additive server fields pass through" promise. That promise is what made 1
 *    and 2 invisible, and it is exactly what a strict response schema exists to
 *    stop being invisible. Both signatures are now gone from the types as well,
 *    and both interfaces declare the fields above.
 *
 * Two timestamp fields on this surface are NOT ISO-8601: `pastDueAt` here and
 * `createdAt` on a ledger entry are selected raw, so they arrive as the Data
 * API's `"YYYY-MM-DD HH:MM:SS"` text. They are validated as non-empty strings,
 * deliberately, and the inconsistency is reported (on the types, at the field)
 * rather than encoded as if intended. `whoami.limits.pastDueAt` — the same
 * concept — IS ISO-8601 with a `Z`, because that route formats it.
 *
 * `workspaceId` on a ledger entry and on the three admin routes is a RAW id, not
 * the public `wsp_<hex>` form `whoami` and the MCP-server records carry.
 */
import * as z from "zod/mini";
import {
  describeResponse,
  responseObject,
  wireBoolean,
  wireEnum,
  wireLiteral,
  wireNonEmptyString,
  wireNumber,
  wireString
} from "./response-common.js";

/**
 * `GET /billing`.
 *
 * `planKey` is the raw `plan_key` column, NOT normalised to the
 * `free | pro | team` union the way `whoami.limits.planKey` is — so it is a
 * string here and an enum there, for the same concept.
 */
export const BillingSummaryResponseSchema = describeResponse(
  "BillingSummaryResponse",
  "Workspace billing summary: prepaid balance, month-to-date spend, cap and plan state.",
  responseObject({
    balanceUsd: wireNumber,
    // The plan catalog was demolished (billing WS5): admission is decided by card
    // presence, not by a subscribed plan, so `planKey`, `subscriptionStatus` and
    // `pastDueAt` went with it. Recorded because a strict schema still expecting
    // them would fail C4 against the current server.
    admissionState: wireNonEmptyString,
    autoTopupEnabled: wireBoolean,
    paymentMethodStatus: wireEnum(["none", "active"]),
    accountType: wireEnum(["standard", "internal"]),
    monthSpendUsd: wireNumber,
    spendCapUsd: wireNumber
  })
);

export const BillingLedgerEntrySchema = describeResponse(
  "BillingLedgerEntry",
  "One signed credit-ledger row. Top-ups are positive, run charges negative.",
  responseObject({
    id: wireNonEmptyString,
    entryType: wireNonEmptyString,
    amountUsd: wireNumber,
    currency: wireNonEmptyString,
    sessionId: z.nullable(wireString),
    workspaceId: z.nullable(wireString),
    description: z.nullable(wireString),
    createdBy: wireString,
    createdAt: wireNonEmptyString
  })
);

export const BillingLedgerResponseSchema = describeResponse(
  "BillingLedgerResponse",
  "Recent credit-ledger rows, newest first. Bounded by `limit`; not cursor-paged.",
  responseObject({ entries: z.array(BillingLedgerEntrySchema) })
);

/** `POST /billing/checkout` and `POST /billing/portal` both answer exactly `{ url }`. */
export const BillingHostedSessionResponseSchema = describeResponse(
  "BillingHostedSessionResponse",
  "A hosted checkout or billing-portal session. The client should open `url`.",
  responseObject({ url: wireNonEmptyString })
);

export const AdminBillingTopupResponseSchema = describeResponse(
  "AdminBillingTopupResponse",
  "Operator credit grant. `inserted` is false on an idempotent replay; " +
    "`balanceUsd` is the authoritative post-state either way.",
  responseObject({
    ok: wireLiteral(true),
    workspaceId: wireNonEmptyString,
    orgId: wireNonEmptyString,
    amountUsd: wireNumber,
    inserted: wireBoolean,
    balanceUsd: wireNumber
  })
);

export const AdminBillingPaymentMethodResponseSchema = describeResponse(
  "AdminBillingPaymentMethodResponse",
  "Operator payment-method override, echoing the applied status.",
  responseObject({
    ok: wireLiteral(true),
    workspaceId: wireNonEmptyString,
    paymentMethodStatus: wireEnum(["none", "active"])
  })
);

export const AdminBillingAccountTypeResponseSchema = describeResponse(
  "AdminBillingAccountTypeResponse",
  "Operator account-type override, echoing the applied type.",
  responseObject({
    ok: wireLiteral(true),
    workspaceId: wireNonEmptyString,
    accountType: wireEnum(["standard", "internal"])
  })
);

export type BillingSummaryResponse = z.infer<typeof BillingSummaryResponseSchema>;
