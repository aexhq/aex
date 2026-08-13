---
title: Model-provider transport and catalog simplification
description: Replace the hand-rolled provider adapters and catalog metadata with rig as the sole runtime transport authority, and generate identity/limits facts from a vendored, hash-pinned models.dev snapshot at build time, while keeping the brain/agent loop and BYOK custody untouched.
keywords:
  - provider gateway
  - model catalog
  - rig
  - models.dev
  - BYOK
  - simplification
audience: implementation agents and maintainers
status: proposal
related:
  - references/architecture.md
  - references/backlog.md
  - references/glossary.md
  - references/model-provider-library-simplification-2026-08-13/README.md
---

# Model-provider transport and catalog simplification

**Amended 2026-08-13.** Decisions 1, 6, and 7, the "Catalog trust is a separate
decision" section, and the open questions are superseded by
[`references/model-provider-library-simplification-2026-08-13/design-2026-08-13.md`](model-provider-library-simplification-2026-08-13/design-2026-08-13.md)
(owner dispositions of review findings F1–F11, revised decisions, spikes, and
the implementation plans in that folder). Everything else below remains valid
proposal material.

Initial design proposal. Not accepted. This document proposes replacing the
hand-rolled provider transport and most of the signed-catalog *content* with two
mature external authorities — **rig** (runtime transport) and **models.dev**
(build-time facts) — while keeping the brain/agent loop and the BYOK credential
layer unchanged. It separates three concerns that are currently conflated in one
hand-authored artifact, and it deliberately leaves the *trust* mechanism of the
catalog as an open decision (see "Catalog trust is a separate decision").

## Context and problem

`aex-brain-provider-gateway` hand-implements eight dialect adapters
(`openai`, `anthropic`, `deepseek`, `zai`, `moonshotai`, `google`,
`openrouter`, `vercel_ai_gateway`), the SSE framing, streaming budgets, the
in-call retry policy, and a `SendGate`/`DispatchProof` machine that proves
whether a request was sent. `aex-model-catalog` hand-implements a signed,
content-addressed catalog whose `ModelEntry` carries roughly twenty fields of
per-model dialect, capability, limit, sampling, reasoning, structured-output,
tool, cache, usage, stop-reason, and error metadata.

The adapter code and the per-model metadata both answer a question that a mature
library already answers better: *how do I speak to this model?* Maintaining them
is dialect churn that a pre-launch team should not own. The goal is to delete as
much of that surface as possible while keeping the parts that are genuinely ours:
admission policy, durable execution, the MicroVM sandbox, and BYOK custody.

## Goals and non-goals

**Goals**

- Replace the eight adapters plus SSE/budget/retry with one maintained library.
- Generate the model *identity and limits* facts instead of hand-authoring them.
- Minimize error classes: typos, wrong dialect, wrong limits, serving unvetted models.
- Keep the public wire contract and the brain's `ProviderPort` seam stable.

**Non-goals**

- Do **not** replace the brain or the agent loop (see "rig does not replace the brain").
- Do **not** change the BYOK credential model or KMS custody (`aex-secret-aws`,
  `aex-secret-custody-dynamodb`, `aex-secret-domain`).
- Do **not** write a custom rig provider to bridge rig↔models.dev gaps.
- Do **not** surface a capability matrix unless a product requirement forces it.

## Three authorities, one per concern

A single source of truth is the wrong shape. Three concerns, three owners:

| Concern | Source of truth | Notes |
| --- | --- | --- |
| **Facts** — which models exist, exact IDs, context window, max tokens | **models.dev**, vendored + hash-pinned, consumed at build time | Never capabilities; never behavior. |
| **Policy** — which pairs we admit and serve | **the operator**, a small code-reviewed allowlist | models.dev knows what exists, not what we vet. |
| **Behavior** — what actually works at runtime | **rig**, the executing code | The only authority that cannot be wrong about what the binary does. |

The mismatch hazard (models.dev lists a feature rig does not implement) is
removed by never *gating* a request on a models.dev field and never advertising a
capability rig cannot perform. The supported-models/providers table is still
encoded (identity + limits + admission), so "is this supported?" is answered
upfront, not by runtime failure. Per-model capability flags exist in models.dev
but are deferred until a product surface needs them; when built, they are the
*intersection* of models.dev's model flags and a small hand-curated rig-provider
capability table, never a runtime guess.

## models.dev schema (verified 2026-08-12)

`https://models.dev/api.json` is an object keyed by provider id (~180 providers).
Each provider carries `doc`, `env`, `id`, `name`, `npm` (the `@ai-sdk/*` package,
whose name encodes the dialect class), `api` (the base URL), and a `models` object
keyed by provider-native model id. Each model entry carries `id`, `name`, `family`,
`attachment`, `reasoning`, `reasoning_options`, `tool_call`, `structured_output`,
`temperature`, `modalities` (input/output), `open_weights`, `limit`
(`context`/`output` tokens), and `cost` (`input`/`output`/`cache_read`/
`cache_write`). There is no per-model `provider` field; the provider is the parent
key.

The `api` field carries the base URL only for `@ai-sdk/openai-compatible` and
gateway-style providers (e.g. `deepseek` `https://api.deepseek.com`, `zai`
`https://api.z.ai/api/paas/v4`, `moonshotai` `https://api.moonshot.ai/v1`,
`openrouter` `https://openrouter.ai/api/v1`). It is `null` for first-party native
providers (`openai`, `anthropic`, `google`) because those packages — and rig —
hardcode their own endpoints.

Mapping to rig is therefore data-driven, not a per-provider table:

- Model identity: transparent — the models.dev `id` is the exact provider-native
  string rig passes through verbatim; no translation.
- Dialect + endpoint: derived from `npm` (dialect class) and `api` (base URL). A
  fixed three-branch rule maps the dialect class to a rig client; the base URL is
  read from `api` when present, otherwise it is rig's own hardcoded origin (native
  providers) or the single `vercel` gateway constant. No per-provider exceptions.
- Capability: only if advertised; the intersection of models.dev model flags and a
  hand-curated rig-provider capability table.

Gateway providers are an exception to *catalog shape*, not routing: `openrouter`
(349 entries) and `vercel` (326) are routing catalogs of `<upstream>/<model>`
strings, not a finite admit set, and need distinct catalog treatment.

## Decisions

1. **Adopt `rig` as the sole runtime transport authority.**
   Delete the eight adapters, `sse`, `stream`, `budget`, and the
   `SendGate`/`DispatchProof` machine. `rig` owns dialect, endpoints,
   capabilities, limits, retry, and normalization. The brain calls one thin
   `RigProviderRouter` implementing the existing `ProviderPort`.

2. **Vendor + hash-pin a models.dev snapshot; codegen at build time.**
   A script fetches models.dev, writes a JSON snapshot plus its SHA into the
   repository, and a small generator emits a Rust module from the vendored file
   (never the network). CI verifies the hash; a scheduled job proposes bumps as
   reviewed PRs. Builds stay hermetic and reproducible.

3. **The generated artifact contains identity, limits, and admission.**
   `provider, model, context_window_tokens, max_output_tokens, admitted`, sourced
   from models.dev and intersected with the operator allowlist. No capability bits
   for now; the capability matrix (models.dev flags ∩ rig-provider table) is added
   later only if a product surface displays it. A `ModelId` newtype validates
   request strings against the compiled set, so a typo fails fast at the boundary.

4. **Keep the brain and agent loop.**
   `rig` replaces `aex-brain-provider-gateway`, not `aex-brain-app` /
   `aex-brain-domain`. The durable-effect model, dispatch tickets, fences,
   cancellation, session state machine, MicroVM tools, telemetry, and retained
   generation stay ours. `rig`'s `Agent` is not adopted: it is stateless and
   in-process, and adopting it would discard durability, sandboxing, and billing.

5. **Keep BYOK custody unchanged.**
   `aex-secret-aws`, `aex-secret-custody-dynamodb`, `aex-secret-domain`, and the
   decrypted-key cache remain. The only change is the hand-off: the decrypted
   `ProviderApiKey` is passed to rig's client instead of being turned into a
   sensitive header by our transport. This relaxes the compile-time
   "adapter cannot see the credential" invariant (the plaintext now reaches a
   third-party library); accepted under the minimize-code goal.

6. **No custom providers; route by dialect class.**
   rig 0.41.0 ships native modules for seven of the eight (`openai`, `anthropic`,
   `gemini`, `deepseek`, `zai`, `moonshot`, `openrouter`) and none for
   `vercel_ai_gateway`; the native modules for the OpenAI-compatible providers are
   thin delegates onto rig's generic OpenAI-compatible completion model. Route by
   models.dev's own dialect class so endpoints come from one authority and each
   dialect has one code path: native modules for `openai`, `anthropic`, `google`;
   the OpenAI-compatible chat-completions builder (rig openai
   `CompletionsClientBuilder` + `base_url` from models.dev's `api` field) for
   `deepseek`, `zai`, `moonshotai`, `openrouter`, and `vercel_ai_gateway`. Two code
   paths, driven by models.dev's `npm` field. A custom provider remains a last
   resort and is disfavored.

7. **Single-attempt dispatch; disable stream reconnection.**
   rig has no request-level retry in production (`reqwest-retry` is dev-only), but
   its SSE layer reconnects a dropped stream with exponential backoff, unbounded by
   default (`DEFAULT_RETRY` has `max_retries = None`). Set the retry policy to
   `max_retries = Some(0)` so a dropped stream is a terminal error, not a silent
   re-send. The `NotSent`/`PossiblySent` distinction is gone; the durable-effect
   layer records send-fate as best-effort. This retains the "no surprise duplicate
   generation" property.

8. **Emergency disable: rebuild-and-redeploy for pre-launch.**
   With the catalog compiled in, withdrawing a model is a code release. Accept
   that now; add a small runtime denylist (DynamoDB key or feature flag) only if
   an incident demonstrates the need.

## Owner decisions (2026-08-12)

- **Catalog trust: A.** Delete the signed-catalog authority (KMS key, OIDC role,
  protected lane, last-good binding, predecessor chain); admission becomes a
  compiled-in allowlist generated from models.dev. Includes removing the
  `AEX_MODEL_CATALOG_BINDING_JSON` step from the `brain-mux` build.
- **Capabilities: included, and they drive behavior.** The generated catalog keeps
  the per-model `tool_call` flag. When a model is `tool_call=false`, the brain
  transparently registers no tools and runs text-only. Every routed path is
  tool-capable (verified), so the only text-only case is a genuinely tool-incapable
  model.
- **Retry: disable stream reconnection.** rig retries only SSE reconnection (no
  request-level retry); set `max_retries = Some(0)`.
- **Provider set: all eight**, including `zai` and the two gateways. Gateway
  admission shape (explicit upstream subset vs pass-through) is deferred to
  implementation.

## Blast radius

**Delete:** the eight adapters; `sse.rs`, `stream.rs`, `budget.rs`; the
`SendGate`/`SendState`/`DispatchProof` machinery; the per-model
`usage_map`/`stop_reason_map`/`reasoning`/`tool_policy`/`cache_policy` metadata.

**Keep:** BYOK custody crates; the decrypted-key cache; the brain app/domain and
its `ProviderPort`; the operator allowlist; a `ModelAdmission` check.

**Add:** a vendored models.dev snapshot + SHA; a `scripts/update-models` refresh
job; a build-time codegen step; CI hash verification; the `ModelId` newtype.

## Validation spikes (2026-08-12)

Run inline (no subagent tool available); each spike is a compile/read-only
experiment against the live models.dev API and rig 0.41.0 source.

- **Spike 1 — models.dev data.** Confirmed the schema (identity, limits, cost,
  capability flags, and a `npm` field encoding the dialect class). All eight
  providers present with current provider-native ids (`deepseek-v4-pro`,
  `claude-opus-4-5`, `gemini-2.5-flash`, `glm-4.6`, `kimi-k2.5`).
- **Spike 2 — rig capability surface.** rig 0.41.0 has native modules for seven of
  the eight; only `vercel_ai_gateway` lacks one. The OpenAI-compatible providers
  (`deepseek`, `zai`, `moonshot`, `openrouter`) are thin delegates onto rig's
  generic OpenAI-compatible `GenericCompletionModel`, which is tool-capable and
  streams. An earlier reading that `zai` was text-only was a shallow-grep artifact
  (the tool logic lives in the delegated generic model, not the provider file); no
  capability gap was found among the eight.
- **Spike 3 — compile check.** rig 0.41.0 builds on the pinned Rust 1.97.1
  toolchain. Provider paths are `rig::providers::{openai, anthropic, gemini,
  deepseek, ...}`; rig 0.41.0 splits into `rig-core` + `rig-agent` + `rig`. The
  openai client supports `.base_url(...)` / `OPENAI_BASE_URL`, so the
  OpenAI-compatible providers are reachable without custom providers.
- **Spike 4 — codegen.** Generated the admit-set shape (`provider, model, context,
  output`) from the vendored snapshot. Tool-capable subset: openai 38, anthropic 13,
  google 21, deepseek 4, zai 14, moonshotai 10.

Conclusion: dialect-class routing from models.dev (three native, five
OpenAI-compatible) is the right shape — one authority for endpoints, one code path
per dialect. The models.dev ∩ rig mismatch concern remains valid in principle (a
library can lag a model's capabilities), but no concrete gap was found among the
eight providers; the earlier `zai` "gap" was a shallow-grep artifact and has been
retracted.

## Catalog trust is a separate decision

The signed-catalog *content* is what this document proposes to delete. The signed
catalog's *authority* — the KMS P-256 signing key, the GitHub OIDC publisher
role, the protected `model-catalog-publish.yml` lane, the immutable last-good
binding, the predecessor chain, and revision-based emergency disable (recorded
in the now-deleted `model-catalog-authority.md`; its decisions are folded into
[`model-provider-library-simplification-2026-08-13/README.md`](model-provider-library-simplification-2026-08-13/README.md)) — is a distinct,
already-accepted subsystem. Two coherent endings:

- **A (full replacement):** delete the authority too; admission becomes
  "compiled-in, code-reviewed." Stronger tamper-evidence (the code is the
  authority), deletes a signature subsystem, but model changes require a release
  and emergency disable is a rebuild.
- **B (thin the signed catalog):** keep the signing/chain/last-good machinery but
  shrink the signed document to the five-field admit set plus the emergency-disable
  list, with content generated from models.dev. Preserves runtime emergency disable
  and the existing publication lane at the cost of keeping the authority subsystem.

Decided: **A** (full replacement). models.dev feeds the *content* identically in
either ending; the choice was purely the trust mechanism. **A** also touches the
release pipeline, not just the catalog crate: `AEX_MODEL_CATALOG_BINDING_JSON` is
today compiled into `brain-mux`, so the removal includes dropping that binding
step.

## Error ledger

| Error class | Killed by |
| --- | --- |
| Typo'd provider/model ID | Build-time codegen + `ModelId` validated against the compiled set |
| Wrong dialect/routing | rig's tested provider mapping |
| Wrong limits (context/max tokens) | models.dev exact values, compiled in, used to pre-validate |
| Serving an unvetted model | operator allowlist (code review) |
| Corrupt/non-reproducible catalog | vendored + hash-pinned snapshot, CI-verified |
| Silent feature mismatch (models.dev vs rig) | capability matrix is the models.dev∩rig intersection, never a models.dev-only claim; unsupported features fail loud |
| Emergency withdrawal | runtime denylist, or rebuild (pre-launch) |
| Duplicate generation | single-attempt dispatch |

## Open questions

- Pin the exact rig version and its provider/base-URL API surface at first
  implementation, and treat rig upgrades as conformance events.
- Gateway admission shape for `openrouter`/`vercel` (explicit upstream subset vs
  pass-through) — to be decided at implementation time.

## Alternatives considered

See the accompanying alternatives exploration for the full space. In brief:

- Status quo (hand-rolled adapters + signed catalog).
- `async-openai` only, plus two native adapters for Anthropic and Gemini.
- `genai` instead of `rig`.
- A self-hosted or managed gateway (LiteLLM / Portkey / OpenRouter / Vercel AI
  Gateway) collapsing all dialects to OpenAI format.
- No catalog at all.
- Keeping the signed catalog and only thinning its content.
- Adopting a Rust agent framework to replace the brain (rejected on
  durability/sandbox grounds).
