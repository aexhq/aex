// TODO(cross-stream): aex-payment-contracts defines this envelope in Rust
// (command::PaymentCommandEnvelope), but nothing generates TypeScript from it, so there
// is nothing this file can import. The contracts stream owes the generator, not the
// type.
export interface PaymentCommandEnvelope {
  readonly schemaVersion: string;
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
      readonly kind: "create_portal_session";
      readonly effect: string;
      readonly organization: string;
      readonly returnUrl: string;
      readonly customer: string;
    }
  | {
      readonly kind: "charge_saved_method";
      readonly effect: string;
      readonly organization: string;
      readonly amount: number;
      readonly customer: string;
      readonly method: string;
    }
  | {
      readonly kind: "lookup_effect_outcome";
      readonly effect: string;
      readonly expect: string;
    }
  | {
      readonly kind: "refund_charge";
      readonly effect: string;
      readonly original: string;
      readonly amount: number;
    };

export interface Succeeded {
  readonly outcome: "succeeded";
  readonly effect: string;
  readonly objectId: string;
  readonly objectType: string;
  readonly status: string;
  readonly amountCents: number;
  readonly currency: "usd";
  readonly providerRequestId?: string;
  readonly apiVersion: string;
}

// TODO(cross-stream): aex-payment-contracts has no PaymentCommandResult. Its outcome
// type is result::PaymentResult, whose failure half is result::PaymentFailure carrying a
// result::PaymentFailureClass, and whose ambiguous half is result::UnknownEvidence. As
// above, no TypeScript is generated from any of it.
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
  readonly reason: "timeout" | "provider_5xx" | "connection_reset";
  readonly providerRequestId?: string;
}
