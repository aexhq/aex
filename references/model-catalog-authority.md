---
title: Model-catalog authority bootstrap and publication
description: The checked-in boundary for the protected model-catalog signer, immutable collection acquisition, GitHub bindings, and the external prerequisites that keep publication fail-closed.
keywords:
  - model catalog
  - KMS
  - bootstrap
  - publication
  - trust root
  - conformance
audience: release maintainers
status: accepted
related:
  - references/rules.md
  - references/develop.md
  - references/repository-hygiene.md
  - references/ghcr-visibility-bootstrap.md
  - .github/workflows/_build-artifacts.yml
  - tests/live/aex-live-model-catalog/src/main.rs
  - runtimes/brain-mux/build.rs
  - tools/aex-release-tool/src/artifact.rs
---

# Model-catalog authority bootstrap and publication

This document records the boundary between checked-in automation and the
owner-controlled authority needed to make `brain-mux` serviceable. A catalog
entry becomes `Active` only after the provider conformance receipt is earned,
the canonical document is signed by the dedicated publisher key, and the
runtime-equivalent verifier accepts the resulting collection. Missing or
partial inputs are release blockers; this workflow must never turn a staged
fixture, an application encryption key, or a guessed provider key into trust.

## Existing authority surfaces

The implementation already has one vocabulary and one verification path:

- [`aex-model-catalog`](../crates/aex-model-catalog/) owns the document,
  detached-envelope, collection, receipt, chain, and trust-root schemas.
- [`aex-model-catalog-publisher`](../tests/live/aex-live-model-catalog/src/main.rs)
  exposes `prepare`, `record-signature`, and `assemble-genesis`. It prepares
  `aex-model-catalog/v1\n || document` as a SHA-256 digest for KMS
  `MessageType=DIGEST`, admits only `ECDSA_SHA_256`, and runtime-verifies the
  genesis collection before emitting bytes.
- [`brain-mux` build binding](../runtimes/brain-mux/build.rs) copies the exact
  trust-root set and collection into the binary. Runtime environment variables
  cannot relabel or replace them.
- [`artifact plan`](../tools/aex-release-tool/src/artifact.rs) requires the
  all-or-none build binding for a published `brain-mux`, validates canonical
  trust roots and both SHA-256 identities, and refuses a path outside the
  checkout.
- [`_build-artifacts.yml`](../.github/workflows/_build-artifacts.yml) now
  acquires the collection from a same-repository immutable GitHub release
  asset into .tmp/model-catalog/collection.json. The download is optional
  for non-publishing builds, but URI and digest are all-or-none and the
  release tool remains the final canonical/schema/signature gate.

The checked-in workflow does not create a key, call KMS, make a provider
request, or update GitHub configuration. It only transports a collection that
an owner has already published and binds its exact bytes to the existing
planner.

## Required GitHub configuration

The following names are the contract. The two public release variables are
read by the main-push artifact workflow; values are non-secret identities, not
credentials:

| Scope | Name | Meaning |
| --- | --- | --- |
| repository variable | `AEX_MODEL_CATALOG_TRUST_ROOTS_JSON` | Canonical `aex.model-catalog-trust-roots.v1` JSON containing only the KMS public SEC1 key(s) |
| repository variable | `AEX_MODEL_CATALOG_TRUST_ROOTS_SHA256` | `sha256:` identity of those exact JSON bytes |
| repository variable | `AEX_MODEL_CATALOG_COLLECTION_URI` | HTTPS URL of the immutable collection asset in `aexhq/aex` |
| repository variable | `AEX_MODEL_CATALOG_COLLECTION_SHA256` | `sha256:` identity of the downloaded collection bytes |

`AEX_MODEL_CATALOG_COLLECTION_FILE` is deliberately no longer a repository
variable. The workflow derives the stable workspace-relative path
.tmp/model-catalog/collection.json after it downloads and verifies the URI;
a runner-specific path cannot become release identity.

A future protected signer should use the dedicated GitHub Environment
`aex-model-catalog-publisher`, with required reviewers, `main` as its only
deployment branch, and self-review disabled where the repository plan allows
it. Its non-secret environment variables are:

| Name | Meaning |
| --- | --- |
| `AEX_MODEL_CATALOG_AWS_REGION` | AWS region containing the publisher key; launch architecture currently selects `eu-west-1` |
| `AEX_MODEL_CATALOG_AWS_ROLE_ARN` | Dedicated GitHub OIDC role ARN |
| `AEX_MODEL_CATALOG_KMS_KEY_ARN` | Exact asymmetric KMS key ARN used for `GetPublicKey` and `Sign` |
| `AEX_MODEL_CATALOG_KMS_KEY_ID` | Stable logical `keyId` placed in the trust-root document and detached signature record |
| `AEX_MODEL_CATALOG_EXPECTED_AWS_ACCOUNT_ID` | Account allow-list passed to the AWS credentials action |

The environment secret `AEX_MODEL_CATALOG_PUBLISH_CONFIRMATION` is a second,
presence-only operator authority for that future workflow. Its value must be a
random non-empty string, must never be printed or copied into an artefact, and
must not be treated as a signing key. The protected environment review remains
the human approval boundary.

The current repository has none of these catalog variables, environment, or
secret. Setting empty placeholders would only move the failure and is not
bootstrap.

## AWS prerequisite, owned outside this repository

An owner must provision and independently review one dedicated asymmetric KMS
key before the protected signer can run:

1. The key must have `KeySpec=ECC_NIST_P256`, `KeyUsage=SIGN_VERIFY`, and
   `SigningAlgorithms` containing `ECDSA_SHA_256`. The signer may call only
   `kms:GetPublicKey`, `kms:Sign`, and (for identity checking)
   `kms:DescribeKey` on that exact key ARN. No application encryption key,
   alias selected by convention, or symmetric key is an acceptable substitute.
2. The key policy must allow the dedicated role these operations and no broad
   KMS mutation. Key-policy and IAM-policy permissions are both required by
   AWS KMS; an IAM allow alone is not sufficient when the key policy does not
   delegate it.
3. The role trust policy must accept GitHub's OIDC provider with audience
   `sts.amazonaws.com` and the subject for the protected environment:
   `repo:aexhq/aex:environment:aex-model-catalog-publisher`. The repository's
   current OIDC settings use the default, name-based subject and have not
   opted into immutable subject claims; if that setting changes, the trust
   policy must be updated to the exact subject GitHub reports before any run.
4. The workflow must pass the expected AWS account to
   `aws-actions/configure-aws-credentials` and must never accept long-lived
   access keys. The existing platform release workflows pin that action at
   `ec61189d14ec14c8efccab744f656cffd0e33f37` (v6.1.0); a future signer must
   use the same reviewed pin or a separately reviewed replacement.

The KMS public key is the only key material that may leave AWS. The bootstrap
must validate the complete `GetPublicKey` metadata and encode the returned
uncompressed P-256 SEC1 point into the canonical trust-root JSON. The private
key never enters GitHub, a variable, a secret, a log, or a release asset.

## One-time genesis sequence

The one-time owner-controlled sequence is:

1. Earn a complete 23-probe conformance receipt for at least one real
   customer-owned `(provider, model)` pair. A provider model-list response is
   not evidence and cannot promote an entry.
2. Produce a canonical `CatalogDocument` whose `required_adapter_source`
   matches the current tree-wide adapter digest and whose entry is `Active`
   only because that receipt proves every declared capability. The document
   must be supplied from a reviewed source identity; no fixture promotion is
   allowed.
3. From the protected environment, read the exact KMS public key, prepare the
   publisher's digest request, call KMS `Sign` with `MessageType=DIGEST` and
   `SigningAlgorithm=ECDSA_SHA_256`, record the closed response, and run
   `assemble-genesis` with the canonical trust roots. The command must stop on
   any metadata, digest, signature, adapter, receipt, time-gate, or
   serviceability mismatch.
4. Publish the collection as a new immutable GitHub release asset. Never
   replace an existing tag or asset, and retain the publication binding as
   non-secret evidence.
5. After independent review, set the four repository variables above. The
   main workflow then downloads the exact asset, checks its SHA-256, and the
   existing release tool and runtime verifier check its canonical collection and
   signatures again.

No checked-in step may silently perform step 1 or 2. Until all five steps have
real evidence, public main publication must continue to fail at
`model-catalog-build-binding-missing` or its more specific planner rule.

## Steady-state publication is not yet implementable

The current publisher intentionally emits only a genesis collection. A normal
revision must append to the verified predecessor chain and carry the exact
sorted set of catalog pins still needed by live sessions. That retention set is
owned by the deployed platform, not by a public Git checkout. A future
steady-state workflow therefore needs, as separate reviewed inputs:

- an attested predecessor collection URI and digest;
- an attested still-live session-pin snapshot, including its source release and
  freshness bound;
- a new canonical document with a strictly advancing sequence and predecessor
  digest; and
- the same dedicated KMS key (or an explicitly reviewed overlapping rotation)
  plus the exact trust-root set used to verify every retained artifact.

Until the platform exports that retention authority and the publisher grows a
chain-extension command that consumes it, no workflow may accept a hand-written
pin list, drop old revisions to fit the 32-revision bound, or reset the chain by
reusing `assemble-genesis`. Those shortcuts would make existing sessions
unreadable or manufacture continuity. This is an external authority blocker,
not a reason to weaken the public release gate.

## Fail-closed checks

The acquisition step and planner must fail when:

- the URI and collection digest are only partially configured;
- the URI is not an HTTPS release asset in this repository, or carries a
  query, fragment, or parent-path component;
- the downloaded bytes do not equal the configured SHA-256 identity;
- trust roots are missing, non-canonical, unsorted, malformed, or do not
  contain valid lowercase P-256 SEC1 points;
- the collection is not canonical JSON, has an invalid chain/signature/receipt,
  is bound to another adapter digest, or contains no serviceable `Active`
  model; or
- a build tries to publish `brain-mux` without all exact inputs.

The workflow never logs credential values, the KMS public-key response body,
provider keys, signed URLs, or GitHub secret values. A failed candidate stays
failed and receives a new immutable identity after a fix-forward.
