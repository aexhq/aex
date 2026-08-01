// TODO(cross-stream): replaced by aex-payment-contracts::ProviderEventEnvelope at merge
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
