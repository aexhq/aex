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
