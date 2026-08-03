---
title: Providers and model catalog — what landed
description: The implemented state of aex-model-catalog, aex-brain-provider-gateway and tests/live/aex-live-model-catalog. Records the canonical vocabulary Brain re-exports, the reconciliations taken against plan 08, what is deliberately deferred, and what each peer stream must change.
keywords:
  - byok
  - providers
  - model catalog
  - sse
  - conformance
  - credentials
audience: implementation agents and maintainers
status: accepted
last_verified: 2026-08-03
related:
  - references/rewrite/contracts.md
  - references/rewrite/test-architecture.md
---

# Providers and model catalog — what landed

Plans of record: `references/rust-native-rewrite-2026-07-31/plans/08-providers-byok.md` and `references/rust-native-rewrite-2026-07-31/plans/07-brain-core.md`, in the parent workspace.

Three packages: `crates/aex-model-catalog` (pure), `crates/aex-brain-provider-gateway`
(the adapter), and `tests/live/aex-live-model-catalog` (the conformance harness).

Six providers, no gateway, no `OpenRouter`, no arbitrary base URL, no
cross-provider fallback, no model-name inference. `anthropic` is canonical and
`anthrophic` is a decode error, not a synonym.

---

## 1. Implemented

### `aex-model-catalog`

| Module | What it owns |
| --- | --- |
| `primitives` | `BoundedString<N>`, `Blake3Digest`, `ModelSlug`, `ToolCallId`, `ToolName`, the base64 serde for opaque bytes |
| `wire_pending` | `CatalogRevision` only — everything else now comes from `aex-wire` |
| `canonical` | the provider-neutral result vocabulary (§3) |
| `failure` | `ProviderFailureKind` (15) and `ProviderFailureClass` (4), with `class()` as the single translation site |
| `document` | the catalog document schema, every struct `deny_unknown_fields`, every string enum closed |
| `receipt` | the 23-probe registry, `ProbeOutcome` with no `Skipped` member, `ConformanceReceipt` |
| `signature` | the detached envelope, the compiled trusted-key set, verification |
| `qualified` | `QualifiedModel` and `CatalogError`, with its `ErrorCode` mapping |
| `catalog` | `Catalog::{load, qualified, admit, entry, disabled, durable_operation_support}` |
| `fixture` | deterministic document construction for tests, the publishing tool and the live harness |

Two invariants are worth naming because they are the reason the crate exists:

- **The receipt gate is a document load invariant, not a runtime check.** An
  `Active` entry whose declared `CapabilitySet` has any capability without a
  passing probe cannot be loaded at all. There is no runtime check to forget,
  because there is no runtime check.
- **Every receipt is bound to an `AdapterSourceDigest`.** Editing an adapter and
  shipping the old catalog fails at process start, not at the first customer
  request.

The launch document ships every `(provider, model)` pair `Staged`, so a fresh
`brain-mux` admits zero models until live evidence exists (OD-24).
`Catalog::active_len()` answers `0` and a test pins it.

### `aex-brain-provider-gateway`

| Module | What it owns |
| --- | --- |
| `wire_pending` | temporary memory-reservation and ciphertext-reference shapes; Brain ports and secret generations are re-exported from their owners |
| `error` | `ProviderFailure`, `RateLimitFeedback`, `RateLimitSource`; canonical failure kinds/classes/details are re-exported from `aex-model-catalog` |
| `redact` | the bounded, credential-safe redactor |
| `sse` | the incremental bounded SSE decoder |
| `budget` | `StreamBudget`, `BudgetOverrun`, `BudgetLedger` |
| `transport` | `WireRequest`, `AuthScheme`, `SendGate`, `Dispatched`, `SendState` |
| `pool` | `IsolationKey`, `ClientPool`, `PooledClient` |
| `credential` | binding model, the two consumed ports, `DenyAllCredentialDirectory`, `ProviderApiKey`, `CredentialCache` |
| `adapter` | the `ProviderAdapter` trait, `DialectState`, `RequestBuildError`, `FrameOutcome`, `FrameDecodeError` |
| `openai` `anthropic` `deepseek` `zai` `moonshotai` `google` | the six dialect adapters |
| `build_identity` | one compile-time source-tree identity for the complete six-adapter build; runtime environment variables cannot relabel it |
| `catalog_port` | bounded signed-envelope loading into an immutable content-addressed revision cache |
| `router` | the total six-provider route, credential affinity/revocation, isolated pool, in-call retry, bounded stream, durable response-start evidence, sealing and receipt |

Three properties are structural rather than conventional:

- **A provider module cannot see a credential.** `WireRequest.auth` is
  `AuthScheme`, an enum carrying only a compiled discriminant. There is no
  variant holding key material and no constructor taking any. The shared core
  turns the tag into a `HeaderValue` with `set_sensitive(true)` at the single
  send site and never hands it back (D-21).
- **`DispatchProof::NotSent` is producible only while the `SendGate` is held.**
  `SendGate` is `#[must_use]`, `pub(crate)`-constructible, and consumed exactly
  once. `SendState::send` refuses a second consume, so "no second generation"
  is a type error. Everything after the consume is `PossiblySent`, *including*
  `reqwest` connect errors: a pooled HTTP/2 connection may already have carried
  the request head and no provider in this set offers a way to ask (D-13).
- **An ignored frame is not proof of generation.** Anthropic's `ping` and
  DeepSeek's `: keep-alive` are `FrameOutcome::Ignored`. Only a decoded dialect
  frame calls `DialectState::mark_started`, which returns `ResponseStarted`
  exactly once, because `mark_response_started` is a durable write that must
  mean "the provider is generating".

### `tests/live/aex-live-model-catalog`

The probe registry (derived from `ProbeId::ALL`, never hand-listed), the
per-provider key resolution, `ReceiptBuilder`, `earns_active` and the staging
diff. `ReceiptBuilder::build` refuses a receipt missing any probe run;
`stage` only ever emits `Staged`.

---

## 2. Deferred, with the reason

| Gap | How it is handled |
| --- | --- |
| Release injection of the whole-tree adapter digest | Per-module identities were removed. `build_identity` accepts only the compile-time `AEX_PROVIDER_ADAPTER_SOURCE_DIGEST` stamp and both catalog loading and router construction refuse a missing or malformed stamp. The release/conformance builder still has to calculate and inject the same digest. |
| `anthropic.rs` pins an offline P-256 public key and signature in its test module | `openai.rs` was migrated to `fixture::qualified`; `anthropic.rs` still loads a signed fixture document, which breaks loudly (`"re-sign it if the document shape changed"`) if `document.rs` or `fixture::entry` moves. Migrating it is a mechanical follow-up now that `fixture::qualified` exists. |
| Z.AI's path is recorded two ways in plan 08 §5.4 | The row gives the base as `https://api.z.ai/api/paas/v4` and the path as `POST /paas/v4/chat/completions`, which cannot both be right. The adapter follows the explicit path, producing `https://api.z.ai/paas/v4/chat/completions`. Probe P-01 settles it before any Z.AI pair can go `Active`; until then every Z.AI entry is `Staged`, so nothing dispatches. |
| `decode`, `finish` and `classify_http` are not handed the `QualifiedModel` | Each adapter therefore compiles its own stop-token and error tables rather than reading `entry.stop_reason_map` / `entry.error_map`. For these six dialects both are provider-invariant, and `anthropic.rs` asserts the compiled table and the catalog's copy agree. But it means the catalog's copies are documentation for the decode path rather than its source of truth. Widening the trait to take the model would make them authoritative. |
| The 23 probes need a real customer key per provider | The `[[test]]` targets stay undeclared and the manifest keeps `not_applicable.targets` naming OD-07. The harness is compiled and unit-tested; adding the target is one change. `ProviderKeys::require` panics with the variable name, proved by a `#[should_panic]` case. |
| No `(provider, model)` pair can ship `Active` | Every launch entry is `Staged` with an `unearned()` receipt — a positive record that the evidence has not been earned, not an absence. |
| The `pcr_` binding table, its routes and the KMS decrypt adapter | Owned by the regional secret stream (OD-23). This crate defines `ProviderCredentialDirectory` and `ProviderCredentialDecryptor`, consumes them, and ships `DenyAllCredentialDirectory` / `DenyAllCredentialDecryptor`. There is **no** plaintext-from-environment path — not disabled, absent. |
| `resolve_unknown` | Returns `UnknownResolution::NoDurableOperation` for all six. Implemented, not stubbed: no provider in this set documents a result lookup for a completed streaming generation. Anthropic is stateless; OpenAI's `GET /v1/responses/{id}` requires `store: true`, which AEX disables; Gemini Interactions is not the launch dialect. |
| Brain session credential pin | The composed router and its full `dispatch_pinned` path exist. The public `ProviderPort` fails `NotSent` because `ResolvedAgentConfig` does not carry an immutable `SessionCredentialPin`; dispatching without it would re-resolve mutable default state. |
| Brain content hydration | Canonical requests carry inline user turns. A configured system reference or placed user block fails before dispatch because the application has no content-hydration port yet. |
| `trybuild` type-level leak test | The workspace has no `trybuild` dependency. The same property is asserted by construction — `ProviderApiKey` implements none of `Clone`, `Debug`, `Display`, `Serialize`, `Deref`, and `WireRequest` has no field that can hold one — plus runtime cases over `Debug` output, error bodies and receipts. Adding `trybuild` is a workspace-manifest change and belongs to whoever owns that decision. |
| `miri` over the `credential` module | Not run: the module contains no `unsafe` and the crate forbids it, so `miri` would add build time without a proposition to test. |

---

## 3. The exact `canonical` surface Brain re-exports

`aex_model_catalog::canonical` is the cross-stream artefact. `aex-brain-domain`
adds `aex-model-catalog` as a dependency and **re-exports** these, never
redefines them (D-CANON, `00-orchestrator-conventions.md` §8a).

```rust
// content
pub enum CanonicalBlock { Text { text, annotations }, Reasoning(ReasoningBlock),
    ToolUse { id, name, input }, ToolResult { call, content, is_error },
    Refusal { text } }
pub struct ReasoningBlock { pub body: ReasoningBody, pub token: Option<ReasoningToken> }
pub enum ReasoningBody { Text { text }, Summary { text }, Redacted }
pub struct ReasoningToken { pub provenance: ProviderId, pub bytes: bytes::Bytes }
pub enum ToolResultPart { Text { text }, Json { value } }
pub struct TextAnnotation { pub start: u32, pub end: u32, pub kind: AnnotationKind }
pub enum AnnotationKind { Citation, Quote }

// messages
pub enum Role { User, Assistant }
pub struct CanonicalMessage { pub role: Role, pub blocks: Vec<CanonicalBlock> }
pub struct CompleteAssistantMessage { pub blocks, pub stop_reason, pub provider,
    pub model, pub catalog, pub proof }
pub struct CompleteProof(pub aex_wire::ContentHash);
pub enum StopReason { EndTurn, ToolUse, MaxOutputTokens, StopSequence, Refusal }
pub enum SealError { EmptyBlocks, UnbalancedToolUse, DuplicateToolCallId,
    RefusalWithoutContent, ReasoningTokenMissing, ReasoningProvenanceMismatch,
    BlockLimit, InvalidToolInputJson, InconsistentUsage }
pub fn seal(blocks, stop, usage, model) -> Result<CompleteAssistantMessage, SealError>;

// usage
pub struct NormalizedUsage { pub input_tokens, pub cache_read_input_tokens,
    pub cache_write_input_tokens, pub output_tokens, pub reasoning_tokens,
    pub tool_use_prompt_tokens, pub provider_total_tokens, pub completeness }
pub enum UsageCompleteness { Exact, Partial(UsageFieldSet), Absent }
pub struct UsageFieldSet(pub u16);  pub enum UsageField { .. }

// request
pub struct CanonicalModelRequest { .. }   // NOT Serialize; see §4
pub struct SystemBlock { pub text, pub cacheable }
pub enum ToolChoice { Auto, None, Required, Named { name } }
pub struct CanonicalToolDef { pub name, pub description, pub input_schema, pub strict }
pub enum ReasoningRequest { Disabled, ProviderDefault, Enabled { budget_tokens, effort } }
pub enum ReasoningEffort { Minimal, Low, Medium, High, Max }
pub enum StructuredOutputRequest { JsonObject, JsonSchema { name, schema, strict } }
pub enum CacheBreakpoint { AfterSystem, AfterTools, AfterMessage { index } }
pub struct CorrelationId(pub BoundedString<64>);

// preview — no From/Into to or from any canonical type exists anywhere
pub enum PreviewFrame { BlockStart, TextDelta, ReasoningDelta, ToolCallStart,
    ToolArgumentsDelta, BlockStop, InterimUsage }
pub enum PreviewBlockKind { Text, Reasoning, ToolUse, Refusal }

// receipt
pub struct ProviderReceipt { .. }
pub struct CredentialBindingRef { pub id, pub revision, pub generation }
pub struct ReceiptRateLimit { .. }  pub enum RateLimitSource { .. }
pub struct ReceiptBounds { .. }
```

Also published for Brain and `regional-session-api` admission:

```rust
aex_model_catalog::{QualifiedModel, CatalogError, Catalog, CatalogHead, CatalogLoadError,
    ProviderFailureKind, ProviderFailureClass, CatalogRevision, BoundedString, ModelSlug,
    ToolCallId, ToolName, Blake3Digest};
aex_model_catalog::document::{ModelEntry, ModelLimits, CapabilitySet, Capability,
    Dialect, EndpointPin, EntryState, DurableOperationSupport, ...};
```

`CatalogError::error_code()` is the `ErrorCode` mapping: `UnknownProvider`,
`UnknownModel`, and `UnqualifiedProviderModel` for the other five arms.

---

## 4. Decisions taken beyond plan 08

| ID | Decision | Why |
| --- | --- | --- |
| D-29 | `ProviderFailureKind` lives in `aex-model-catalog::failure`, not the gateway's `error` module; the gateway re-exports it | A catalog entry's `error_map` is *data* that names these kinds. Putting the vocabulary above the document that references it keeps the catalog self-describing and keeps the gateway out of the pure crate's dependency graph. The plan's `error::ProviderFailureKind` path still resolves. |
| D-30 | Catalog signatures are **ECDSA P-256 / SHA-256 over ASN.1 DER**, not Ed25519 | `00-orchestrator-conventions.md` OD-21 pins `ECDSA_SHA_256` wherever the signing key lives in AWS KMS, and KMS has no Ed25519 key spec. This supersedes plan 08 D-02. The signature bytes are therefore variable-length DER, bounded at 80 bytes, not a fixed `[u8; 64]`. The signed input keeps its `aex-model-catalog/v1\n` domain-separation prefix. |
| D-31 | `CompleteProof` wraps `aex_wire::ContentHash` (SHA-256); only the **catalog digest** is blake3 | The workspace's content-hash wire type is SHA-256 and the proof is an exported field. `references/rust-native-rewrite-2026-07-31/plans/00-orchestrator-conventions.md` §4 reserves blake3 for Merkle pages; the catalog digest is one (it chains revisions), the completeness proof is not. |
| D-32 | `canonical::ToolCallId` is `BoundedString<128>`, **not** `aex_wire::ToolCallId` | `aex_wire::ToolCallId` is an AEX-minted prefixed UUID. A provider-assigned id — Moonshot's `search:0`, OpenAI's `call_abc123`, Anthropic's `toolu_01…` — cannot be expressed in it, and the plan requires carrying the provider's id verbatim. |
| D-33 | `CanonicalModelRequest` is not `Serialize`/`Deserialize`; `digest()` hashes a projection | It holds a `QualifiedModel`, which is a live handle into a loaded catalog revision rather than a wire value. The projection collapses the selection to `(provider, model, catalog)` and is what gets hashed and exported. |
| D-34 | `Catalog::load` also rejects an entry whose `in_call_status_retry` names a status outside `{429, 503}`, or more than three attempts | D-20 restricts in-call retry to statuses that are definitive non-generation rejections. Leaving that as an adapter-side check would let a mistaken publish ask for a retry that could create a second generation; making it a load error means the document simply cannot express it. |
| D-35 | The SSE decoder joins `data:` lines by append-newline-then-strip, per the SSE grammar, rather than by separator | A separator join silently drops an empty `data:` line. An empty payload line is a real frame shape, not padding. |
| D-36 | The redactor masks any run of ≥ 20 credential-alphabet characters containing a digit, in addition to the keys AEX currently holds | A provider can echo a key AEX is *not* dispatching under — a customer's other key, pasted into a prompt. Redacting only known secrets would miss it. Truncation runs after masking, so a bound can never leave a prefix of a secret behind. |
| D-37 | `StreamBudget::narrowed_to` lets a catalog entry only **shrink** a shared-safety default, never widen one | Otherwise a mistaken publish could raise the process's own memory ceiling from data. |
| D-38 | `aex-model-catalog` ships an always-on `fixture` module rather than a feature-gated one | `aex_wire::testing` sets the precedent, and rule 9 of the test architecture forbids test-only build features. Nothing in `fixture` can promote an unproved entry: `entry()` always produces `Staged` with an `unearned()` receipt, and `promote()` is an explicit call that a document invariant still re-checks at load. |

---

## 5. Changes needed from peers

> **Status correction, 2026-08-01.** Every peer named below has landed, and none of
> these requests was honoured. `aex-brain-domain` did **not** take the
> `aex-model-catalog` dependency; it defines its own `wire_pending` copies of the
> canonical vocabulary and every one of them has since diverged from
> `aex_model_catalog::canonical`. `aex-brain-application` landed `ProviderPort` and
> friends bound to *those* copies, so the gateway's restatement is no longer verbatim
> and the two sides of the port name different request, message, usage and receipt
> types. `aex-brain-application::pressure` is an empty placeholder, so
> `MemoryReservation` and `ReservationClass` exist nowhere. `aex-secret-domain`'s
> `CiphertextRef` and `EncryptionContext` are different designs from the gateway's,
> not different spellings. The per-item markers in
> `aex_brain_domain::wire_pending` and `aex_brain_provider_gateway::wire_pending`
> now record each divergence against a path that resolves; the list below is the
> original ask, kept for provenance.

- `TODO(cross-stream): aex-brain-domain adds aex-model-catalog as a dependency and
  re-exports aex_model_catalog::canonical::* — CanonicalBlock, CanonicalMessage,
  CanonicalModelRequest, CompleteAssistantMessage, CompleteProof, StopReason,
  NormalizedUsage, PreviewFrame, ProviderReceipt, ReasoningToken — rather than
  redefining any of them (D-CANON).`
- `TODO(cross-stream): aex-brain-application owns ProviderPort, CatalogPort,
  DispatchTicket, CancelToken, PreviewSink, DispatchProof, DispatchStage,
  DispatchEvidence, EffectIdentity, ProviderOutcome, ProviderDispatchError,
  UnknownResolution, BoxFuture, MemoryReservation and ReservationClass. They are
  restated verbatim in aex-brain-provider-gateway::wire_pending and every one is
  marked for replacement at merge.`
- `TODO(cross-stream): aex-brain-application may widen ProviderFailureClass to the
  fifteen ProviderFailureKind members. Until then ProviderFailureKind::class() is
  the single translation site.`
- `TODO(cross-stream): aex-secret-domain owns SourceGeneration, RevocationEpoch,
  CiphertextRef and EncryptionContext; they are restated in the gateway's
  wire_pending and marked for replacement.`
- `TODO(cross-stream): the regional secret stream owns the pcr_ binding table, its
  registration/list/revoke routes and the KMS decrypt adapter, and implements
  ProviderCredentialDirectory and ProviderCredentialDecryptor. Until then brain-mux
  binds DenyAllCredentialDirectory.`
- `TODO(cross-stream): the contracts stream renders CatalogRevision as
  mc1_<hex of blake3-256> and adds no other catalog field to the public wire.
  aex_wire::CatalogRevision does not exist yet; it lives in
  aex_model_catalog::wire_pending.`
- `TODO(cross-stream): aex-wire's ToolCallId cannot carry a provider-assigned call
  id. If the contracts stream intends it to, it needs a second, bounded-string
  grammar; otherwise Brain re-exports aex_model_catalog::ToolCallId (D-32).`
- `TODO(cross-stream): aex-brain-test-support hosts the provider_fake module this
  stream owns; the peer owns the crate manifest.`
- `TODO(cross-stream): runtimes/brain-mux composes CompositeCatalog { models:
  Arc<Catalog>, tools: Arc<ToolCatalog> } to satisfy CatalogPort, and calls
  ClientPool::close plus CredentialCache::invalidate on every revocation wake.`

---

## 6. Toolchain note

`aws-lc-sys` does not build on this Windows host without NASM. Every build in
this stream ran with `AWS_LC_SYS_PREBUILT_NASM=1`, which uses the prebuilt
objects shipped in the crate. CI on Linux is unaffected; this is recorded so a
reviewer does not read a local failure as a workspace defect. As plan 08 §12
already notes, `cargo +stable` is also the working local invocation, because the
`rust-toolchain.toml` pin resolves to a rustup toolchain without a `cargo`
component.
