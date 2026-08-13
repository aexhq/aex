---
title: Model-provider library simplification — amendments, spikes, and implementation plans
description: Follow-up to the 2026-08-12 proposal: owner decisions on the review findings, four verification spikes, and the phased implementation plans. Supersedes the affected sections of the original proposal.
keywords:
  - provider gateway
  - model catalog
  - rig
  - models.dev
  - simplification
audience: implementation agents and maintainers
status: accepted
last_verified: 2026-08-13
related:
  - references/model-provider-library-simplification-2026-08-12.md
  - references/model-catalog-authority.md
  - references/rewrite/providers.md
---

# Model-provider library simplification — follow-up

This folder records the second round of the design: the original proposal
[`model-provider-library-simplification-2026-08-12.md`](../model-provider-library-simplification-2026-08-12.md)
was reviewed against the live codebase (six exploration passes) and the rig
0.41.0 / models.dev facts (one external verification pass). Eleven review
findings (F1–F11) were dispositioned by the owner on 2026-08-13. Four spikes
then verified the amended design. Implementation plans are compiled here;
implementation itself is deferred to a later session.

| Document | What it is |
| --- | --- |
| [`design-2026-08-13.md`](design-2026-08-13.md) | **Read first.** Owner decisions on F1–F11 and the revised decisions 1–8. Supersedes the affected sections of the 08-12 proposal. |
| [`spikes/rig-streaming-retry.md`](spikes/rig-streaming-retry.md) | Working rig 0.41.0 prototype: client construction, streaming, errors, retry, and the zero-reconnect verdict. |
| [`spikes/models-dev-codegen.md`](spikes/models-dev-codegen.md) | Vendored models.dev snapshot facts and the generated admit-table schema (801 rows, 162 KiB). |
| [`spikes/minimal-catalog-surface.md`](spikes/minimal-catalog-surface.md) | The minimal `QualifiedModel`/allowlist surface and every call-site change it implies. |
| [`spikes/deletion-touchpoints.md`](spikes/deletion-touchpoints.md) | The complete phase-grouped deletion and rewiring inventory, including platform-repo coupling. |
| [`implementation-plans.md`](implementation-plans.md) | **The plans.** Six phases, per-phase touchpoints, verification gates, lanes, and cut points. |

## Headline results

- **rig 0.41.0's OpenAI-compatible streaming is already zero-reconnect**: the
  infinite-reconnect machinery in rig's event source is unreachable through the
  public provider path (one error item, then the stream ends). The original
  decision 7's `max_retries = Some(0)` was targeting an API that does not
  exist — no custom `CompletionModel` or fork is needed.
- **Retry stays ours and bounded**: a router-level loop (429/503 definitive
  refusals plus transport drops, ≤3 equal-jitter attempts, cancellable) around
  rig's call, ~50–80 LOC; exhaustion returns `Terminal` so runs stop
  deterministically.
- **The stream budget is deleted** (owner: no bounds — the `ProviderPort`
  `budget` parameter is removed outright, including the mux's 1 MiB
  stream-buffer permit), and the generated table shrinks to identity +
  context/output limits + `tools`/`parallel_tools` + dialect class.
- **Clean split**: `aex-model-vocabulary` (pure canonical/failure/primitives)
  and a slimmed `aex-model-catalog` (generated admit table + two-arm
  `QualifiedModel` admission); the `unqualified_provider_model` wire code is
  deleted. The new transport crate is `aex-brain-provider`.
- **`aws-lc-rs` leaves the pure crate** with `signature.rs`, which also removes
  the reason `aex-session-app` copies `ModelQualifier`; it depends on the
  slimmed catalog directly.
- **Credential ports move into `aex-brain-provider-custody`**; the
  decrypted-key cache lands in `aex-brain-provider` (consumer side), keeping
  custody stateless.
- **Snapshot refresh is manual** (maintainer + CI hash check, normal release
  train); no cron, no PR bot.

Original proposal status: superseded in its decisions 1, 6, 7 and its
"Catalog trust" section by [`design-2026-08-13.md`](design-2026-08-13.md),
which also carries the second-round owner dispositions (gateway admission
breadth, terminal retry semantics, crate naming, and the remaining open-item
closures); unchanged sections remain valid.
