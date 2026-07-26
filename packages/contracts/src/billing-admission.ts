/**
 * The admission vocabulary the money gates decide with.
 *
 * A LEAF module, importing nothing, for the same reason `schemas/runtime-kind.ts`
 * is one: four modules need this list — `runtime-types.ts` for `WhoAmI.limits`,
 * `account-types.ts` for `BillingSummary`, and the two response schemas that
 * check both against the wire — and any of them owning it would make the other
 * three import a much larger module for one constant, or force a cycle between a
 * schema and the types derived from it.
 *
 * Contrast an allowance `dimension`, which is deliberately an OPEN string: the
 * dimensions a free allowance is denominated in are hosted billing policy, and a
 * closed copy of them in the public package would be the duplication the prepaid
 * model was built to remove. These three are not policy. They are the whole
 * public answer to "what sized this workspace", they change only when the
 * product does, and a client switching on them wants the compiler to tell it
 * when a fourth appears.
 */

/**
 * How the money gates size a workspace. There is no plan catalog: a saved card
 * and the auto-recharge flag are the only inputs, so the state is derived rather
 * than sold.
 *
 * - `free` — no card on file. Free monthly allowances only.
 * - `carded_manual` — a card is saved; credit is bought a top-up at a time.
 * - `carded_auto` — a card is saved and auto-recharge is on.
 */
export const BILLING_ADMISSION_STATES = ["free", "carded_manual", "carded_auto"] as const;

export type BillingAdmissionState = (typeof BILLING_ADMISSION_STATES)[number];
