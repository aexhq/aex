---
title: Model-catalog authority and publication
description: Protected signing, immutable publication, atomic last-good consumption, and non-authoritative compatibility monitoring.
keywords:
  - model catalog
  - KMS
  - publication
  - trust root
  - compatibility
audience: release maintainers
status: accepted
related:
  - references/rules.md
  - references/develop.md
  - references/repository-hygiene.md
  - references/ghcr-visibility-bootstrap.md
  - .github/workflows/_compile-artifacts.yml
  - .github/workflows/_build-artifacts.yml
  - .github/workflows/model-catalog-publish.yml
  - infra/modules/model-catalog-authority/README.md
  - release/model-catalog/README.md
  - runtimes/brain-mux/build.rs
  - tools/aex-release-tool/src/artifact.rs
---

# Model-catalog authority and publication

The signed model catalog is static reviewed compatibility metadata. It records
the provider-native model identity and the closed capabilities and limits that
the current adapter/runtime contract knows how to consume. It is not a provider
health report, availability promise, qualification receipt, or copy of a
provider model-list response.

Provider reachability can change independently of a release. Neither a failed
live probe nor a missing provider credential may block ordinary public CI,
publication of reviewed compatibility metadata, or consumption of the current
last-good signed collection.

## Authority surfaces

- [`aex-model-catalog`](../crates/aex-model-catalog/) owns the document,
  detached-envelope, collection, and trust-root schemas and their verifier.
- [`aex-model-catalog-publisher`](../tests/live/aex-live-model-catalog/src/main.rs)
  generates a canonical document from one reviewed `*.source.json`, validates
  that source/document pair, prepares only its signing digest, records the KMS
  signature, and assembles a runtime-verified collection. With a verified
  predecessor it appends exactly one revision and retains every prior pin.
- [`model-catalog-publish.yml`](../.github/workflows/model-catalog-publish.yml)
  is the protected authority lane. It runs automatically only after a merge to
  `main` changes one `release/model-catalog/*.source.json`, and can also be
  dispatched manually with an exact current-main source path and digest.
- [`_compile-artifacts.yml`](../.github/workflows/_compile-artifacts.yml)
  consumes one repository variable, validates its closed canonical shape,
  materializes the exact trust roots and immutable collection, then exports the
  existing release-tool environment variables. The release tool and runtime
  remain the final canonical, signature, chain, and serviceability gates. That
  workflow compiles `brain-mux`; the publishing half in
  [`_build-artifacts.yml`](../.github/workflows/_build-artifacts.yml) never
  reads the binding, because the catalogue is already inside the bytes.
- [`model-catalog-authority`](../infra/modules/model-catalog-authority/) creates
  the dedicated P-256 key and exact OIDC role. The private platform composition
  owns the AWS environment while this public repository owns the reusable
  authority contract.

The ordinary main workflow never calls KMS or a model provider and never
updates GitHub configuration. A candidate publisher or external monitoring
failure leaves `AEX_MODEL_CATALOG_BINDING_JSON` unchanged, so subsequent builds
continue to consume the last independently reviewed binding.

## Atomic build binding

The public artifact workflow reads exactly one non-secret repository variable:

| Scope | Name | Meaning |
| --- | --- | --- |
| repository variable | `AEX_MODEL_CATALOG_BINDING_JSON` | Canonical compact `aex.model-catalog-build-binding.v1` JSON containing the exact trust-root JSON and digest plus the immutable collection URI and digest |

The value has this closed shape:

```json
{"collectionSha256":"sha256:<64 lowercase hex>","collectionUri":"https://github.com/aexhq/aex/releases/download/<immutable-tag>/<content-addressed-asset>.json","schema":"aex.model-catalog-build-binding.v1","trustRootsJson":"<canonical aex.model-catalog-trust-roots.v1 JSON>","trustRootsSha256":"sha256:<64 lowercase hex>"}
```

The build lane rejects an open, non-canonical, oversized, partial, off-repository,
or digest-inconsistent value before exporting these established internal inputs:
`AEX_MODEL_CATALOG_TRUST_ROOTS_JSON`,
`AEX_MODEL_CATALOG_TRUST_ROOTS_SHA256`,
`AEX_MODEL_CATALOG_COLLECTION_URI`,
`AEX_MODEL_CATALOG_COLLECTION_SHA256`, and the derived workspace-relative
`AEX_MODEL_CATALOG_COLLECTION_FILE`. Those are implementation inputs, not
independently mutable repository variables.

Publication emits the exact canonical build-binding file as an immutable,
content-addressed release asset. The protected workflow deliberately has no
credential that can mutate repository variables. Until a dedicated narrowly
scoped GitHub App credential exists, an owner must independently review the
published asset and replace `AEX_MODEL_CATALOG_BINDING_JSON` in one operation.
Never copy its fields into separate variables.

That separate promotion step provides last-good behavior:

1. A source merge or manual dispatch creates a candidate immutable release.
2. Failure at generation, validation, signing, assembly, publication, or
   readback does not alter the installed binding.
3. Ordinary CI continues using the previously installed collection.
4. Only independent review followed by one atomic variable replacement makes
   the candidate the new build input.

Absence is allowed for non-publishing builds and for the first sequence-1
publication. A published `brain-mux` still requires the complete binding.

## Protected publisher configuration

The GitHub Environment `aex-model-catalog-publisher` is restricted to `main`
and supplies these non-secret values:

| Name | Meaning |
| --- | --- |
| `AEX_MODEL_CATALOG_AWS_REGION` | AWS region containing the publisher key; currently `eu-west-1` |
| `AEX_MODEL_CATALOG_AWS_ROLE_ARN` | Dedicated GitHub OIDC publisher role ARN |
| `AEX_MODEL_CATALOG_KMS_KEY_ARN` | Exact asymmetric signing-key ARN |
| `AEX_MODEL_CATALOG_KMS_KEY_ID` | Stable logical `keyId` encoded in roots and signatures |
| `AEX_MODEL_CATALOG_EXPECTED_AWS_ACCOUNT_ID` | Exact account allow-list for credential acquisition |

`AEX_MODEL_CATALOG_PUBLISH_CONFIRMATION` is a presence-only environment secret
and second operator authority. It is not key material or artifact data. The
workflow never creates the Environment or fills missing values.

The key must be customer-managed `ECC_NIST_P256`, have
`KeyUsage=SIGN_VERIFY`, be enabled, non-multi-region, AWS-KMS-origin, and admit
only `ECDSA_SHA_256`. The role may call `DescribeKey`, `GetPublicKey`, and
`Sign` on that exact key. No application encryption key, symmetric key,
long-lived AWS credential, or convention-selected alias is a substitute.

The OIDC trust subject is
`repo:aexhq/aex:environment:aex-model-catalog-publisher`, with audience
`sts.amazonaws.com`, repository `aexhq/aex`, ref `refs/heads/main`, and the
exact workflow name. The public P-256 point is the only key material that may
leave AWS; the private key never enters GitHub.

## Publication and predecessor rules

Each reviewed compatibility source is immutable. The publisher computes one
timestamp before generation, validates the generated canonical document before
OIDC, derives the exact trust root from KMS, signs only the prepared SHA-256
digest with `MessageType=DIGEST`, and read-backs every uploaded asset before
making the prerelease non-draft.

When `AEX_MODEL_CATALOG_BINDING_JSON` is installed, the publisher validates it
and downloads its exact collection before any AWS call. The repository-owned
assembler verifies that predecessor with the current roots, requires the new
document sequence to equal `head + 1`, requires its predecessor digest to equal
the old head, appends the new pin, and conservatively retains all prior pins.
It refuses a 33rd retained revision; the workflow does not invent pruning
authority. Without an installed predecessor the assembler accepts only
sequence 1.

A successful run publishes four never-overwritten assets: trust roots, the
signed collection, the publisher's binding, and the canonical build binding.
Tags include source SHA, workflow run, and attempt. Existing tags or assets are
never overwritten.

## Monitoring is not publication authority

The scheduled/manual compatibility monitoring lane may observe provider
compatibility and availability independently, but monitor output is never called
or consumed
by ordinary CI, the publisher, the build lane, or runtime startup. A failed,
skipped, expired, or unavailable
provider observation cannot remove a signed entry, replace a build binding,
fail a main build, or block release consumption. Compatibility changes are made
only by reviewing a new static source file and passing it through the protected
publisher.

The release-evidence inventory pass likewise exercises only the registered
journey descriptors; it does not contact a provider or the deployed API. The
same journeys run once, with credentials, during the evidence phase.

## Fail-closed checks

Publication or build acquisition must fail when its own input is invalid:

- the reviewed source is untracked, dirty, outside
  `release/model-catalog/*.source.json`, or has the wrong digest;
- generation and independent source/document validation disagree;
- an installed binding is non-canonical, open, partial, oversized, points away
  from this repository's immutable release assets, or has a digest mismatch;
- a predecessor is omitted for a later sequence, fails verification, has the
  wrong head, or would require dropping a retained pin;
- KMS metadata, digest signing, signature recording, assembly, release
  readback, or immutable-name checks fail; or
- a published `brain-mux` lacks a complete verified build binding.

Those failures reject only the candidate operation. They never mutate or
invalidate the installed last-good binding.
