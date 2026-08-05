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
