---
title: release-bound E2E and user evidence
description: Exact-coordinate public workflow and fail-closed prerequisites for earning hosted E2E and user receipts.
keywords:
  - release evidence
  - e2e
  - user tests
  - receipts
  - cleanup
audience: implementation agents and maintainers
status: active
related:
  - references/develop.md
  - references/rules.md
  - references/repository-hygiene.md
---

# Release-bound E2E and user evidence

`.github/workflows/release-evidence.yml` is the public receipt producer. It is
manually dispatched by selecting the immutable `main-<sha>-run-<id>-attempt-<n>`
release tag as the workflow ref. The workflow requires that tag ref and its
resolved commit to match the supplied tag and exact public source SHA. The
dispatch accepts the immutable composition manifest and release-tool URI,
SHA-256 digest and byte size together with their main prerelease tag, semantic
`releaseId`, the exact `dev` or `prd` plane, and the exact
`deployment_context_digest` of the private `VERIFYING` continuation this run
must satisfy. It does not accept receipts or a test command.

The workflow downloads and verifies both public blobs, verifies their GitHub
attestations against the reviewed main producers, validates the manifest, and
binds all coordinates to one tagged source commit. It creates an archive from that
exact checkout so the test sources and Cargo/Bun locks used by the run are
preserved and attested with the receipts.

The continuation digest is not a runtime configuration value. The private
deploy lane mints it only after the exact release reaches its durable
`VERIFYING` fence. The public workflow receives that protected SHA-256 identity
and writes it as `subject.deploymentContextDigest` in both receipts; the private
finalizer must compare it with the same continuation document. The deployed
service proves its public release identity rather than echoing a digest it
never received.

## Protected plane inputs

The selected `aex-release-evidence-<plane>` GitHub Environment owns the remote
test inputs. Both `aex-release-evidence-dev` and
`aex-release-evidence-prd` use the same names:

- secret `AEX_RELEASE_EVIDENCE_API_KEY`;
- variable `AEX_RELEASE_EVIDENCE_API_HOST`;
- variable `AEX_RELEASE_EVIDENCE_PLANE`, set to exactly the Environment's
  `dev` or `prd` suffix; and
- variable `AEX_RELEASE_EVIDENCE_MAXIMUM_BUDGET_MICRO_USD`, set to exactly
  `0` for the current provider-free journeys.

The workflow selects the Environment from a strict `dev|prd` choice, serializes
evidence per plane and release, and refuses an Environment whose configured
plane identity differs from that choice before contacting the plane. The
selected E2E and user journeys use only the protected API key and the API URL
derived from the protected host variable, and incur no provider spend; do not
configure placeholder synthetic-identity or provider-document secrets. Do not
put secret values in workflow inputs, repository variables, fixtures, logs,
JUnit, cleanup ledgers, or docs.

The workflow derives the health URL as
`https://<AEX_RELEASE_EVIDENCE_API_HOST>/api/release/health`; it is not a
separate secret or operator-selected route. The URL must have no credentials,
query, fragment, or redirect. `session-stream-api` alone mounts this public
route; the `/internal/healthz` and `/internal/readyz` routes remain private.
Immediately before the suites and again after they finish, it must return HTTP
200, `application/json`, `Cache-Control: no-store`, and exactly:

```json
{"schema":"aex.release-health.v1","releaseId":"sha256:<64 lowercase hex>","status":"ready"}
```

Another release, another schema, an extra field, or a non-ready response fails
the run. This replaces the former generic authenticated list-route preflight,
which proved reachability but not the deployed release under test.

Before handbook step 7 can run, the repository settings must contain the
`aex-release-evidence-prd` Environment with `AEX_RELEASE_EVIDENCE_PLANE=prd`,
the canonical PRD API host, a PRD-only evidence credential, and the reviewed
zero-spend bound. Its deployment policy must permit only the immutable
`main-<sha>-run-<id>-attempt-<n>` release tags accepted by the workflow. The
same policy applies to dev, including
`AEX_RELEASE_EVIDENCE_PLANE=dev`. Neither Environment may require a human
reviewer: the accepted release handbook makes the evidence suites themselves
the hard gate and forbids human gates in the orchestrated cascade.

## Evidence gates

A release run must first pass `graph verify --release` and produce a non-empty
exact package/target scenario matrix. The public live user suite must also list
at least one test whose name is an exact `live.*` scenario identity; its
registry/descriptor assertion is deliberately not counted as a user journey.

Every selected target runs without a suite-level retry and must produce a
non-empty pre-run inventory, JUnit with the same collected count and no failed,
skipped, todo, retried or flaky case, and a suite-owned hygiene report. The
hygiene report is bound to the release and workflow run and records the
provisioned identities, their cleanup verdicts, cleanup-ledger digest, bounded
spend and secret-canary result. Missing evidence, residue, over-budget spend or
a canary observation fails before receipt construction.

The workflow derives `aex.evidence-receipt.v1` bytes with the public release
tool, verifies their release freshness, attests the two receipt files and the
source archive, and publishes them under a new run/attempt-addressed prerelease.
Both receipts include the exact deployment-context digest and attach the hashed
before/after health observations. Existing assets and tags are never replaced.

## Current fail-closed state

The source now has one graph-selected E2E target, `SC-REGIONAL-ADMISSION`, and
one deployed-plane user journey, `live.registry-list`. Both prove anonymous
denial plus authenticated access through the served workspace-registry route,
emit closed JUnit/inventory output, and report zero-spend cleanup hygiene. The
exact release-health router is mounted only by `session-stream-api`.

The documented dev Environment already has a reviewed credential, canonical
host, zero-spend bound, and immutable-main-release deployment policy. It still
needs the explicit `AEX_RELEASE_EVIDENCE_PLANE=dev` identity and removal of any
required-reviewer rule before this workflow revision can run without a human
gate. The PRD Environment described above is also an external prerequisite;
this source change does not create or populate it. These source and
configuration changes are not a receipt by themselves: the source must first
land on public `main`, be certified into an immutable release, and be deployed
at the private continuation's exact release identity. A selected plane that
does not serve that release and its health contract fails closed before
producing evidence.

All local and CI scratch stays under
`.tmp/release-evidence/<run-id>-<attempt>/`. The ephemeral runner discards it;
only the exact attested source archive and passing receipt files are published.
