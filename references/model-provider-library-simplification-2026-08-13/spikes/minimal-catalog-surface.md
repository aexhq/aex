---
title: Spike — minimal catalog surface and brain-seam impact
description: The thin QualifiedModel wrapper over the generated allowlist, fact provenance for every surviving consumer, and the complete call-site change list.
keywords:
  - catalog
  - brain
  - seam
  - spike
audience: implementation agents and maintainers
status: accepted
last_verified: 2026-08-13
related:
  - references/model-provider-library-simplification-2026-08-13/design-2026-08-13.md
---

# Spike: minimal catalog surface and brain-seam impact

Read-only analysis. Verdict: **Option A — keep `QualifiedModel` as a thin
wrapper over the generated table with identical accessor names.**
~40–60 edited lines outside `aex-model-catalog`, versus ~double for a full
rename (Option B), with zero behavioral gain.

## Fact provenance after deletion

| Consumed fact | Call sites (surviving) | Provenance |
| --- | --- | --- |
| `Capability::Tools` | aex-brain-app/src/activation/run.rs:75 | generated `tools: bool` |
| `Capability::ParallelTools` | ports/tool.rs:124, run.rs:94 | generated `parallel_tools: bool` |
| `limits().max_tools` | run.rs:83-86 | per-dialect-class constant (models.dev lacks it) |
| `limits().max_output_tokens` | run.rs:1525 | generated |
| `limits().context_window_tokens` | aex-brain-domain/src/context.rs:90 | generated |
| `limits().min_cacheable_prefix_tokens` | context.rs:100-101 | per-dialect-class constant |
| `requires_reasoning_token()` | canonical.rs:380 (seal) | per-dialect-class function |
| `DurableOperationSupport` | run.rs:1128, effect.rs recover | always `None`; type moves to aex-brain-domain |
| `CatalogPort::digest` | no production callers | delete the method |
| `state()` / EntryState | gateway (deleted) + catalog_port | deleted — every compiled row is admissible |
| `catalog()` / `CatalogRevision` | seal proof, session pinning | `mc1_<blake3>` of the vendored snapshot digest |

`Capability`/`CapabilitySet` shrink to a 2-bit set but keep
`has(Capability::Tools)` / `has(Capability::ParallelTools)` so
`model_tool_fields` and `allows_parallel_emission` compile unchanged.
`CatalogError` keeps its three-arm vocabulary; `UnqualifiedProviderModel`
becomes unreachable from the compiled table but stays for the wire contract
(409 code) and scripted tests.

## Minimal type sketch

```rust
pub enum Capability { Tools, ParallelTools }
pub struct CapabilitySet(u8);                       // EMPTY, from_slice, has, with

pub enum DialectClass {
    OpenAiResponses, AnthropicMessages, DeepSeekChat, ZaiChat,
    MoonshotChat, GeminiGenerateContent, OpenRouterChat, VercelAiGatewayChat,
}

pub struct AdmittedModel {                          // generated row
    pub provider: ProviderId,
    pub model: ModelSlug,
    pub context_window_tokens: u32,
    pub max_output_tokens: u32,
    pub tools: bool,
    pub parallel_tools: bool,
    pub dialect: DialectClass,
}
impl DialectClass {
    pub const fn max_tools(self) -> u16;
    pub const fn min_cacheable_prefix_tokens(self) -> u32;
    pub const fn requires_reasoning_token(self, has_tool_use: bool) -> bool;
}

pub struct QualifiedModel {                         // thin wrapper (Option A)
    entry: &'static AdmittedModel,
    catalog: CatalogRevision,
}
impl QualifiedModel {
    pub fn provider(&self) -> ProviderId;
    pub fn model(&self) -> &ModelSlug;
    pub fn catalog(&self) -> CatalogRevision;
    pub fn entry(&self) -> &'static AdmittedModel;
    pub fn dialect(&self) -> DialectClass;
    pub fn capabilities(&self) -> CapabilitySet;
    pub fn limits(&self) -> ModelLimits;            // { context_window_tokens, max_output_tokens }
    pub fn requires_reasoning_token(&self, has_tool_use: bool) -> bool;
}

pub enum CatalogError {
    UnknownProvider { provider: ProviderId },
    UnknownModel { provider: ProviderId, model: ModelSlug },
    UnqualifiedProviderModel,
}
pub fn admit(provider: ProviderId, model: &str) -> Result<QualifiedModel, CatalogError>;
pub const SNAPSHOT_REVISION: CatalogRevision;       // mc1_ of vendored snapshot
const _: () = assert!(!ALLOWLIST.is_empty());       // replaces is_service_capable
```

## Call-site churn (Option A)

- **aex-brain-app**: `ports/catalog.rs` −2 trait methods; `run.rs` `support()`
  returns `None` (−5 lines); `memory.rs` `FixedCatalog` −2 methods.
  `model_tool_fields`, `tool.rs`, `context.rs`: **0 edits**.
- **aex-brain-domain**: 0 production edits (`wire_pending` re-exports
  unchanged; `DurableOperationSupport` lands here).
- **brain-mux**: `wake.rs` `AbsentCatalog` −2 methods; `main.rs` +
  `release_catalog.rs` re-bind to the allowlist port; readiness gate deletes
  the `catalog_verified` signature flag (exact line map in the deletion
  spike); test fixture lines rewritten.
- **session-stream-api**: `release_catalog.rs` re-point (~20 lines).
- **aex-session-app**: delete the `ModelQualifier`/`QualifiedModel` copy
  (ports.rs:550-618) — the aws-lc-rs rationale dies; the crate already has a
  legal direct dependency on `aex-model-catalog` (`use_cases.rs` uses it),
  and workspace-check's dependency-direction rule is name-suffix based
  (`-app` may link workspace members, just no vendor SDK).
- **aex-brain-test-support**: 0 edits — `fixture::qualified_entry(provider,
  model, caps)` and `fixture::qualified(entry)` keep their signatures (the
  2-bit set is passed as before); only the internals of `fixture.rs` shrink.

## Dependency purity

`aws-lc-rs` in `aex-model-catalog` is used **only** by `signature.rs`
(ECDSA P-256; there is no `p256` crate in this package — the design doc's
mention was inaccurate). Deleting signature/collection/catalog/receipt also
orphans `sha2` and `bytes`; `serde_json` drops to dev-only. The crate becomes
pure Rust: `aex-wire`, `blake3`, `base64`, `hex`, `serde`, `thiserror`,
`time`.
