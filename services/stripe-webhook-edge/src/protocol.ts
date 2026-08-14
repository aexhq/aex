/** The one provider protocol pin shared by both Stripe edges. */
export const STRIPE_API_VERSION = "2026-06-24.dahlia" as const;

/** The complete endpoint event configuration, in stable order. */
export const HANDLED_EVENT_TYPES = [
  "payment_method.attached",
  "payment_method.updated",
  "payment_method.detached",
  "payment_intent.succeeded",
  "payment_intent.payment_failed",
  "charge.dispute.created",
  "refund.created",
  "refund.failed",
] as const;

export type HandledEventType = (typeof HANDLED_EVENT_TYPES)[number];

/** Exact camel-case projection of `aex_payment_contracts::PaymentCommandEnvelope`. */
export interface PaymentCommandEnvelope {
  readonly schemaVersion: "v1";
  readonly providerIdempotencyKey: string;
  readonly deadline: string;
  readonly command: PaymentCommand;
  readonly metadata: {
    readonly effect: string;
    readonly organization: string;
    readonly kind: string;
    readonly credit: number;
  };
}

export type PaymentCommand =
  | {
      readonly kind: "ensure_customer";
      readonly effect: string;
      readonly organization: string;
      readonly email: string;
    }
  | {
      readonly kind: "create_top_up_checkout";
      readonly effect: string;
      readonly organization: string;
      readonly amount: number;
      readonly successUrl: string;
      readonly cancelUrl: string;
      readonly customer: string;
      readonly tax: "provider_automatic" | "none";
    }
  | {
      readonly kind: "create_payment_method_session";
      readonly effect: string;
      readonly organization: string;
      readonly successUrl: string;
      readonly cancelUrl: string;
      readonly customer: string;
      readonly consentedAt: string;
    }
  | {
      readonly kind: "detach_payment_method";
      readonly effect: string;
      readonly organization: string;
      readonly customer: string;
      readonly method: string;
    }
  | {
      readonly kind: "lookup_effect_outcome";
      readonly effect: string;
      readonly expect: string;
      readonly provider: string | null;
    }
  | {
      readonly kind: "refund_charge";
      readonly effect: string;
      readonly original: string;
      readonly amount: number;
    };

export interface HostedSession {
  readonly url: string;
  readonly expiresAt: string;
}

export interface Succeeded {
  readonly outcome: "succeeded";
  readonly effect: string;
  readonly providerRef: string;
  readonly providerCreatedAt: string;
  readonly hosted: HostedSession | null;
  readonly charged: number;
  readonly tax: null;
}

export type PaymentCommandFailure = Rejected | Indeterminate;

export interface Rejected {
  readonly outcome: "rejected";
  readonly code: string;
  readonly declineCode?: string;
  readonly errorType: string;
  readonly providerRequestId?: string;
  readonly apiVersion: string;
}

export interface Indeterminate {
  readonly outcome: "indeterminate";
  readonly reason: "timeout" | "provider_5xx" | "connection_reset" | "rate_limited";
  readonly status?: number;
  readonly providerRequestId?: string;
}

export interface Failed {
  readonly outcome: "failed";
  readonly effect: string;
  readonly failure: {
    readonly class: "card_declined" | "authentication_required" | "invalid_request";
    readonly providerCode: string | null;
    readonly declineCode: string | null;
    readonly retryable: false;
  };
}

export interface Unknown {
  readonly outcome: "unknown";
  readonly effect: string;
  readonly evidence:
    | { readonly evidence: "timeout"; readonly waitedMs: number }
    | { readonly evidence: "transport_lost" }
    | { readonly evidence: "server_error"; readonly status: number }
    | { readonly evidence: "ambiguous_response"; readonly providerCode: string | null };
}

export type PaymentResult = Succeeded | Failed | Unknown;
