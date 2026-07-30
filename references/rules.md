---
title: public repository rules
description: Non-negotiable rules for public-surface ownership, secrets, tests, Git safety, and release integrity.
keywords:
  - rules
  - public boundary
  - secrets
  - tests
  - git safety
  - release gates
audience: implementation agents and maintainers
status: accepted
related:
  - references/repo.md
  - references/develop.md
  - references/repository-hygiene.md
---

# Public repository rules

These rules override convenience. Read them before editing code, config, tests,
or docs.

## Public product boundary

- This repository owns only public SDK, CLI, contracts, docs,
  package metadata, and user-visible tests. Standalone runnable samples live in
  the sibling `examples/` repository and consume this repository's supported
  public surface.
- Public code and documentation describe supported customer behavior. They must
  not expose private hosted implementation, deployment, billing/rate policy,
  substrate adapters, abuse policy, margins, or reconciliation details.
- Private platform behavior belongs in its owning private repository. Cross-repo
  integration is expressed through public contracts and blackbox behavior.

## Secrets

- Never commit, print, inspect, fixture, or document real provider keys, API
  keys, MCP credentials, signed URLs, tokens, or `.env*` values.
- Example files may list variable names and safe placeholders only. Recorded
  fixtures and diagnostic summaries must be sanitized and reproducible.
- Use approved deterministic environment loaders for live tests. If a secret
  value reaches a transcript or durable artifact, treat it as exposed and
  rotate it.

## Testing

- For new behavior, bug fixes, and regressions, write the first meaningful test
  from requirements, public contracts, docs, logs, or user-visible evidence
  before implementation when practical.
- Prefer user tests as the blackbox layer for SDK, CLI, package, and
  published-artifact behavior. Add narrower unit coverage where it improves
  diagnosis without replacing the public assertion.
- Do not weaken, skip, or bypass tests, package checks, boundary checks, or
  release gates to make a change pass.

## Git and release integrity

- Never add AI attribution, generated-by notes, or AI co-author trailers to
  commits, docs, comments, pull requests, or release text.
- Preserve unrelated working-tree changes. Do not force-push `main`, discard
  work, stage, commit, or push unless the user requests it.
- The public release workflow is the sole publisher of public npm packages.
  Candidate identity and integrity must remain exact across validation and
  promotion; executable workflow and package configuration are authoritative.

## Documentation placement

Repository-wide rules, instructions, procedures, curated logs, and backlogs live
under `references/`; `AGENTS.md` remains an index. Generated diagnostics and
worktrees are transient and follow
[`repository-hygiene.md`](repository-hygiene.md).
