/** The exact handoff accepted by finance-ingest. Keep fixture-tested with Rust. */
export interface IngestRequest {
  readonly request: "provider_event";
  readonly event: ProviderEventEnvelope;
}

export type ProviderEventKind =
  | "payment_method_attached"
  | "payment_method_updated"
  | "payment_method_detached"
  | "payment_intent_succeeded"
  | "payment_intent_payment_failed"
  | "charge_dispute_created"
  | "refund_created"
  | "refund_failed";

export type CardBrand =
  | "visa"
  | "mastercard"
  | "amex"
  | "discover"
  | "diners"
  | "jcb"
  | "unionpay"
  | "unknown";

interface MethodDisplay {
  readonly customer: string;
  readonly method: string;
  readonly brand: CardBrand;
  readonly last4: string;
  readonly expiryMonth: number;
  readonly expiryYear: number;
  readonly providerCreatedAt: string;
}

export type ProviderEventFacts =
  | ({ readonly kind: "payment_method_attached" } & MethodDisplay)
  | ({ readonly kind: "payment_method_updated" } & MethodDisplay)
  | { readonly kind: "payment_method_detached"; readonly method: string }
  | {
      readonly kind: "payment_intent_succeeded";
      readonly organization: string;
      readonly credit: string;
      readonly charged: string;
      readonly intent: string;
    }
  | {
      readonly kind: "payment_intent_payment_failed";
      readonly organization: string;
      readonly intent: string;
      readonly failure: {
        readonly class: "card_declined" | "authentication_required" | "invalid_request";
        readonly providerCode: string | null;
        readonly declineCode: string | null;
        readonly retryable: false;
      };
    }
  | {
      readonly kind: "charge_dispute_created";
      readonly organization: string;
      readonly charge: string;
      readonly amount: string;
    }
  | {
      readonly kind: "refund_created" | "refund_failed";
      readonly organization: string;
      readonly charge: string;
      readonly refund: string;
      readonly amount: string;
    };

export interface ProviderEventEnvelope {
  readonly schemaVersion: 1;
  readonly providerEventId: string;
  readonly object: string;
  readonly kind: ProviderEventKind;
  readonly occurredAt: string;
  readonly facts: ProviderEventFacts;
  readonly rawDigest: string;
  readonly providerApiVersion: string;
  readonly effect?: string;
  readonly receivedAt: string;
}
