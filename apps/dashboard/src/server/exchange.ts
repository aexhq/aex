export interface OAuthClaims {
  readonly provider: string;
  readonly providerAccountId: string;
  readonly email: string;
  readonly emailVerified: boolean;
  readonly name?: string;
  readonly image?: string;
}

export interface IdentityExchangeCommand {
  readonly provider: string;
  readonly providerAccountId: string;
  readonly emailClaims: { readonly email: string; readonly verified: boolean };
  readonly nonce: string;
  readonly callbackId: string;
  readonly issuedAt: string;
}

export function buildIdentityExchange(
  claims: OAuthClaims,
  binding: { readonly nonce: string; readonly callbackId: string; readonly issuedAt: string },
): IdentityExchangeCommand {
  return {
    provider: claims.provider,
    providerAccountId: claims.providerAccountId,
    emailClaims: { email: claims.email.toLocaleLowerCase("en-US"), verified: claims.emailVerified },
    nonce: binding.nonce,
    callbackId: binding.callbackId,
    issuedAt: binding.issuedAt,
  };
}
