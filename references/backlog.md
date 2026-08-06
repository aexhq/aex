---
title: public repository backlog
description: Deferred public-repository work with explicit revisit triggers.
keywords:
  - backlog
  - ci
  - supply chain
audience: maintainers and implementation agents
status: accepted
related:
  - references/develop.md
  - release/README.md
---

# Public repository backlog

## Asynchronous supply-chain assurance

Dependency audits, licence inventory, packaged-artifact SBOM generation, and
vulnerability scanning are deferred during the startup phase. They must not
block pull requests, protected-main publication, dev deployment, or live tests;
scanner availability and newly published provider advisories are external state
that can change after deployment.

Revisit this when a customer or compliance commitment requires the evidence,
or when the team has enough CI capacity to run it asynchronously without
lengthening release feedback. The future lane should record findings for
triage, keep scanner failures non-blocking for dev, and introduce a blocking
rule only for a separately reviewed, demonstrably exploitable runtime-critical
finding.

## Browser MicroVM variants

The `2gb-browser`, `4gb-browser`, and `8gb-browser` image variants are excluded
from the public release authority during prelaunch. The pinned AL2023 ARM64
repository does not publish Chromium, so listing those variants would create
artifacts that deterministically fail in the provider image builder. The five
non-browser variants remain published.

Revisit this when the image build has a digest-pinned ARM64 browser artifact
with verified provenance and a live AWS MicroVM qualification. Keep the browser
runtime contract and local generator code until then; restoring release rows
requires the provider build and browser smoke to pass for every restored shape.
