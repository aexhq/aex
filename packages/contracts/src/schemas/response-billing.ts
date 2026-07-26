/**
 * Response schemas for the `billing.*` and `adminBilling.*` families.
 *
 * Both `BillingSummary` and `BillingLedgerEntry` USED to carry
 * `[key: string]: unknown` — an explicit "additive server fields pass through"
 * promise. That promise is what made two real gaps invisible (`BillingSummary`
 * omitting `accountType`, `BillingLedgerEntry` omitting `workspaceId`), and it
 * is exactly what a strict response schema exists to stop being invisible. Both
 * signatures are now gone from the types as well, and both interfaces declare
 * every field the server sends.
 *
 * One gap remains, resolved in favour of the server (a schema that fails every
 * real response is not a gate): `workspaceId` on a ledger entry and on the three
 * admin routes is a RAW id, not the public `wsp_<hex>` form `whoami` and the
 * MCP-server records carry, so it is validated as a plain string.
 *
 * `createdAt` on a ledger entry is NOT ISO-8601: it is selected raw, so it
 * arrives as the Data API's `"YYYY-MM-DD HH:MM:SS"` text. It is validated as a
 * non-empty string, deliberately, and the inconsistency is reported rather than
 * encoded as if intended. `resetAt` and `blocked.at` ARE ISO-8601 — they are
 * constructed, not selected. `pastDueAt`, the third timestamp that used to sit
 * here, went with the plan catalog.
 */
import * as z from "zod/mini";
import { BILLING_ADMISSION_STATES } from "../billing-admission.js";
import {
  describeResponse,
  responseObject,
  wireBoolean,
  wireEnum,
  wireLiteral,
  wireNonEmptyString,
  wireNumber,
  wireString,
  wireTimestamp
} from "./response-common.js";

/**
 * One free monthly allowance row inside `GET /billing`.
 *
 * `dimension`, `unit` and `label` are validated as non-empty strings rather than
 * as enums on purpose: the dimensions a free allowance is denominated in are
 * hosted billing policy, and pinning them here would put a second copy of that
 * policy in the public package — the exact duplication the prepaid model was
 * built to remove. The SHAPE is what this schema is for.
 *
 * `approximateTokens` rides only on the USD-denominated token allowance, and
 * only when there is usage to infer a model from.
 */
export const BillingAllowanceSchema = describeResponse(
  "BillingAllowance",
  "One free monthly allowance: quota, consumption and the instant it resets.",
  responseObject({
    dimension: wireNonEmptyString,
    quota: wireNumber,
    used: wireNumber,
    remaining: wireNumber,
    unit: wireNonEmptyString,
    label: wireNonEmptyString,
    resetAt: wireTimestamp,
    approximateTokens: z.optional(
      responseObject({ model: wireNonEmptyString, tokens: wireNumber })
    )
  })
);

/** Auto-recharge settings plus the two guards a top-up form has to respect. */
export const BillingAutoTopupSchema = describeResponse(
  "BillingAutoTopup",
  "Auto-recharge settings, the minimum accepted top-up and the daily recharge cap.",
  responseObject({
    enabled: wireBoolean,
    thresholdUsd: wireNumber,
    amountUsd: wireNumber,
    minimumAmountUsd: wireNumber,
    maxPerDay: wireNumber
  })
);

/**
 * `GET /billing`.
 *
 * `planKey`, `subscriptionStatus` and `pastDueAt` are GONE with the catalog they
 * described; a strict schema still expecting them fails C4 against the current
 * server. What replaces them is the prepaid surface: the period, the
 * card-derived `admissionState`, the allowance rows, the auto-recharge block,
 * the saved card, and any live block.
 */
export const BillingSummaryResponseSchema = describeResponse(
  "BillingSummaryResponse",
  "Workspace billing summary: prepaid balance, month-to-date spend, cap, free allowances and card state.",
  responseObject({
    balanceUsd: wireNumber,
    monthSpendUsd: wireNumber,
    spendCapUsd: wireNumber,
    period: wireNonEmptyString,
    admissionState: wireEnum(BILLING_ADMISSION_STATES),
    accountType: wireEnum(["standard", "internal"]),
    paymentMethodStatus: wireEnum(["none", "active"]),
    autoTopupEnabled: wireBoolean,
    blocked: z.nullable(responseObject({ at: wireTimestamp, reason: wireNonEmptyString })),
    paymentMethod: responseObject({
      present: wireBoolean,
      brand: z.nullable(wireString),
      last4: z.nullable(wireString)
    }),
    autoTopup: BillingAutoTopupSchema,
    allowances: z.array(BillingAllowanceSchema)
  })
);

/** `PATCH /billing/autotopup` echoes exactly the stored settings. */
export const BillingAutoTopupResponseSchema = describeResponse(
  "BillingAutoTopupResponse",
  "The stored auto-recharge settings after the update.",
  responseObject({ autoTopup: BillingAutoTopupSchema })
);

/**
 * One row of `GET /billing/statements`.
 *
 * Only ISSUED periods are listed. A month whose statement did not reconcile is
 * withheld rather than rendered on demand, so an absent period is a statement
 * that was never issued — not one this read failed to find.
 *
 * `issuedAt` IS ISO-8601: the handler formats it, unlike the raw ledger
 * `createdAt` above.
 */
export const BillingStatementSummarySchema = describeResponse(
  "BillingStatementSummary",
  "One issued monthly statement: the period, when it was issued, and the five " +
    "figures that reconcile opening balance to closing.",
  responseObject({
    period: wireNonEmptyString,
    issuedAt: wireTimestamp,
    openingBalanceUsd: wireNumber,
    creditsPurchasedUsd: wireNumber,
    usageUsd: wireNumber,
    adjustmentsUsd: wireNumber,
    closingBalanceUsd: wireNumber
  })
);

export const BillingStatementListResponseSchema = describeResponse(
  "BillingStatementListResponse",
  "The months a customer can download, newest first. Bounded server-side; not cursor-paged.",
  responseObject({ statements: z.array(BillingStatementSummarySchema) })
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

/** `POST /billing/topup/checkout` and `POST /billing/portal` both answer exactly `{ url }`. */
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
