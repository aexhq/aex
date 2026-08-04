---
title: public repository hygiene and generated artifacts
description: Placement, redaction, retention, and cleanup rules for public-repo diagnostics, suite output, scratch directories, and worktrees.
keywords:
  - repository hygiene
  - diagnostics
  - suite diagnostics
  - worktrees
  - scratch
  - cleanup
audience: implementation agents and maintainers
status: accepted
related:
  - references/rules.md
  - references/develop.md
---

# Public repository hygiene and generated artifacts

Durable repository-wide rules, instructions, procedures, design/decision
records, curated release logs, research summaries, and backlogs belong under
`references/`. `AGENTS.md` is an index only.

Generated output does not belong in `references/`. Raw diagnostics can contain
credentials, signed URLs, or provider output even when the producer attempts
redaction.

## Approved transient locations

| Use | Location | Retention |
| --- | --- | --- |
| General local scratch and downloaded artifacts | `.tmp/<tool>/<run-id>/` | Delete when the task closes. |
| CI suite capture inside an ephemeral runner | `.suite-diagnostics/` | Raw subtree stays on-runner; upload only explicitly redacted output. |
| Human Git worktrees | `%LOCALAPPDATA%\aex\worktrees\aex\<slug>` | Remove after branch integration or abandonment. |
| Release-controller worktrees | Parent controller-owned worktree root | Controller removes registrations and files after the run. |

Do not create or unpack root-level `.release-diagnostics/`,
`.release-worktrees/`, `.suite-diagnostics-release-<id>/`, or
`release-diagnostics/`. Their ignore rules are defensive protection for
legacy tooling and accidental downloads, not approved storage.

The protected main artifact lane uses the deterministic CI-only path
.tmp/model-catalog/collection.json for a downloaded, digest-bound release
asset. It is not a durable catalog source, is never uploaded as raw scratch,
and is discarded with the ephemeral runner; the collection's immutable release
asset URI and SHA-256 are the only retained identities.

## Curated evidence

Promote evidence to `references/` only when it will remain useful after the
run. Write a dated, human-readable summary containing provenance, outcome, and
non-secret run identifiers. Never promote raw logs, downloaded bundles,
dependency trees, credentials, signed URLs, or copied environment output.

## Cleanup procedure

1. Confirm the absolute target stays inside the intended cleanup root.
2. Check Git tracking/ignore status and inspect registered worktree ownership.
3. Preserve substantive tracked or untracked work on a branch or named stash.
4. Remove registered worktrees through the owning repository and prune Git
   metadata; delete only verified unregistered residue.
5. On Windows, remove reparse points themselves without traversing their
   targets.
6. Verify the active checkout HEAD/status and confirm the target paths and stale
   worktree registrations are gone.

Every new generator must use an approved root and document its redaction,
retention, and cleanup behavior here. Update the repository-hygiene validation
in the same change if a new output class is genuinely required.
