/** The one provider protocol pin shared by both Stripe edges. */
export const STRIPE_API_VERSION = "2026-06-24.dahlia" as const;

/** The complete endpoint event configuration, in stable order. */
export const HANDLED_EVENT_TYPES = [
  "payment_method.attached",
  "payment_method.updated",
  "payment_method.detached",
  "checkout.session.completed",
  "payment_intent.succeeded",
  "payment_intent.payment_failed",
  "payment_intent.canceled",
  "charge.dispute.created",
  "charge.dispute.closed",
  "refund.created",
  "refund.updated",
  "refund.failed",
] as const;

export type HandledEventType = (typeof HANDLED_EVENT_TYPES)[number];
