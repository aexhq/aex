// TODO(cross-stream): aex-payment-contracts defines this envelope in Rust
// (event::ProviderEventEnvelope, over event::ProviderEventKind and
// event::ProviderEventFacts), but nothing generates TypeScript from it, so there is
// nothing this file can import.
export interface ProviderEventEnvelope {
  readonly providerEventId: string;
  readonly eventType: string;
  readonly objectId: string;
  readonly objectType: string;
  readonly createdAtProvider: number;
  readonly apiVersion: string;
  readonly rawBodySha256: string;
  readonly organizationId?: string;
  readonly facts: Readonly<Record<string, string | number | boolean>>;
}
