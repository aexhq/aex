/**
 * Response schemas for the `billing.*` and `adminBilling.*` families.
 *
 * One place the server and the declared types still disagree, resolved in favour
 * of the server (a schema that fails every real response is not a gate):
 * `BillingLedgerEntry` does not declare `workspaceId`; the server sends it on
 * every row (the WS2 cost-attribution tag), `null` for org-level rows. Both
 * interfaces carry `[key: string]: unknown` — an explicit "additive server
 * fields pass through" promise, and that promise is what makes the gap
 * invisible. The index signature is not honoured here.
 *
 * `createdAt` on a ledger entry is NOT ISO-8601: it is selected raw, so it
 * arrives as the Data API's `"YYYY-MM-DD HH:MM:SS"` text. It is validated as a
 * non-empty string, deliberately, and the inconsistency is reported rather than
 * encoded as if intended. `resetAt` and `blocked.at` ARE ISO-8601 — they are
 * constructed, not selected.
 */
import * as z from "zod/mini";
import { BILLING_ADMISSION_STATES } from "../runtime-types.js";
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
 * described. What replaces them is the prepaid surface: the period, the
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
