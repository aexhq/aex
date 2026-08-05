# Model-catalog authority

Creates one dedicated, non-exportable `ECC_NIST_P256` / `SIGN_VERIFY` AWS KMS
key and one GitHub OIDC role for the public protected model-catalog publisher.
The role can inspect only that key and can sign only with `ECDSA_SHA_256`; it
cannot create, rotate, disable, delete, encrypt, decrypt, or administer a key.

The trust policy admits only `aexhq/aex` on `refs/heads/main` through the
`aex-model-catalog-publisher` GitHub Environment and the exact `model catalog
publish` workflow claim. The key policy separately grants the same finite use
operations and keeps administration with exact owner-supplied principals. The
module deliberately has no default account, administrator, or logical key id.

The private platform repository owns the single environment composition root
that instantiates this public module. The KMS key ARN, publisher role ARN, and
logical key id are non-secret protected-environment variables; the private key
never leaves KMS.
