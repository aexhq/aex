---
title: Providers and model catalog — what landed
description: The implemented state of aex-model-catalog, aex-brain-provider-gateway, aex-brain-provider-custody, and tests/live/aex-live-model-catalog. Records the canonical vocabulary Brain re-exports, the reconciliations taken against plan 08, what is deliberately deferred, and what each peer stream must change.
keywords:
  - byok
  - providers
  - model catalog
  - sse
  - conformance
  - credentials
audience: implementation agents and maintainers
status: accepted
last_verified: 2026-08-04
related:
  - references/rewrite/contracts.md
  - references/rewrite/test-architecture.md
---

# Providers and model catalog — what landed

Plans of record: `references/rust-native-rewrite-2026-07-31/plans/08-providers-byok.md` and `references/rust-native-rewrite-2026-07-31/plans/07-brain-core.md`, in the parent workspace.

Four packages: `crates/aex-model-catalog` (pure), `crates/aex-brain-provider-gateway`
(the transport/dialects), `crates/aex-brain-provider-custody` (regional binding and
KMS custody), and `tests/live/aex-live-model-catalog` (the conformance harness).

Eight customer-owned provider authorities: six native providers, OpenRouter and
Vercel AI Gateway. There is no arbitrary base URL, AEX-owned provider key,
AEX-selected cross-provider fallback, or model-name inference. `anthropic` is
canonical and `anthrophic` is a decode error, not a synonym.

### Owner amendment: two customer-owned gateway authorities

On 2026-08-04 the owner expanded the target to the six native providers plus
OpenRouter and Vercel AI Gateway, while retaining customer-owned credentials
only. The code slice now exists end to end: generated public bindings, fixed
catalog dialect/origin identities, two gateway-owned adapters, the total router,
bounded route receipts, source-build identity and the live-key registry. No
gateway model is `Active`: exact model entries still require signed P-01–P-23
evidence earned with customer-supplied test keys before the runtime admits them.

The target wire spellings are `openrouter` and `vercel_ai_gateway`. They must be
new `ProviderId` members, not aliases of `openai` and not arbitrary transport
base URLs. `ProviderId` identifies the authority whose credential AEX resolves,
whose endpoint it calls, whose quota and bill the customer reconciles, and whose
failure/receipt namespace it records. For these paths that authority is the
gateway even when the gateway later chooses an upstream host for the exact
model. Treating either path as `openai` would bind the wrong credential and make
the durable receipt false.

Internally, each gateway has its own fixed `EndpointPin`, chat-completions
`Dialect` member and `ProviderAdapter` over the existing bounded HTTP/SSE
transport. Both gateways publish OpenAI-compatible surfaces, but compatibility is not protocol identity:
paths, routing controls, stream metadata, usage fields, error bodies and request
ids can differ. Common encoding/decoding primitives may be extracted only where
goldens prove the bytes and state transitions are the same; neither adapter may
delegate identity, error classification or receipts to `OpenAiAdapter`. The
official gateway surfaces and routing behavior are documented by
[OpenRouter provider routing](https://openrouter.ai/docs/guides/routing/provider-selection)
and [Vercel AI Gateway](https://vercel.com/docs/ai-gateway).

BYOK here means AEX stores only the customer's OpenRouter key or Vercel AI
Gateway key in the same encrypted, revisioned provider-credential authority used
for native keys. AEX does not provision a shared or managed gateway key. If the
customer configures upstream BYOK inside their gateway account, that remains
between the customer and the gateway. Initial AEX support must not put upstream
provider credentials into a request body: the current one-binding session pin
and credential-hidden `WireRequest` make that leakage impossible. Request-scoped
multi-key gateway BYOK would require a separate custody and revocation design,
not another environment variable.

The gateway contract should admit one exact gateway-native model slug and no
model fallback list. Same-model upstream routing is part of the gateway path the
customer selected and preserves the gateway's reliability value, but the
adapter must normalize any actual model/upstream route metadata the gateway
publishes into a bounded receipt. It must never copy an unbounded vendor metadata
object into the journal. AEX still performs no automatic retry after an
ambiguous send; gateway-internal routing produces at most the one result returned
for that one durable effect.

The implemented correctness-preserving vertical slice is:

1. Both `ProviderId` members are authored in
   `api/schemas/provider/provider.yaml`; the Rust, TypeScript, JSON Schema and
   OpenAPI bindings are generated from it.
2. `aex-model-catalog` owns the two closed origins and two distinct gateway
   chat dialects. Future entries remain `Staged` until reviewed compatibility
   metadata explicitly promotes them in signed source.
3. Two adapters share only bounded OpenAI-compatible chat encoding/decoding;
   each owns its provider identity, fixed path, request-id headers and error
   classification. Both are part of the source-build digest and the router's
   match is total.
4. `ProviderReceipt.gateway_route` stores only a bounded reported model and
   upstream-provider label. The requested gateway/model remain the outer
   receipt identity, and an unbounded vendor metadata object never enters the
   journal.
5. Provider seams and the live key registry include
   `AEX_LIVE_PROVIDER_KEY_OPENROUTER` and
   `AEX_LIVE_PROVIDER_KEY_VERCEL_AI_GATEWAY`; the 23-probe matrix can monitor
   gateway compatibility without controlling catalog admission.
6. Public user documentation is gated on the runtime composition and reviewed
   signed compatibility support, not transient provider availability.

The adapters are usable only through a catalog-qualified pair and keep staged
pairs inadmissible, so widening the enum does not fabricate supported model
metadata. Compile-time exhaustiveness covers the catalog fixture, router and
live-key registry, but remains distinct from external conformance evidence.

The trade-offs are specific. A gateway adds one selected network intermediary
and its latency/outage domain, but offers same-model upstream routing and quota
aggregation. Dedicated adapters and normalized route receipts cost more source
and conformance work, but preserve credential affinity, billing reconciliation
and protocol-drift detection. The hot path gains no new process or sidecar: it
continues to use the mux's shared bounded clients, with pools separated by the
gateway origin and customer credential generation. Gateway quotas and noisy
neighbors remain isolated by the existing provider/workspace scheduler only
when each gateway is represented by its own provider identity. The existing
admin-only lookup by credential id widens from six to eight sequential strongly
consistent point reads because its key omits the provider; runtime dispatch
already carries the provider and remains one point read. A locator/key-shape
clean cut is a separate optimization if that admin path becomes material, not a
reason to alias gateway identities.

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

- **Signed compatibility metadata is admission authority.** An `Active` entry
  loads only when its closed provider, model, dialect revision, compiled
  endpoint, capabilities, limits and retry policy pass the catalog's static
  validation. Provider reachability and transient behaviour are request-time
  concerns, not binary or catalog validity.
- **Live receipts are external assurance evidence.** They may bind an exact
  entry and adapter-source digest for monitoring, but they are not embedded in
  `ModelEntry`, do not expire a catalog and cannot block build or startup.

The launch source explicitly activates the reviewed
`deepseek/deepseek-v4-flash` compatibility entry. A provider outage produces a
typed provider failure; it does not withdraw the signed entry.

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
| `openai` `anthropic` `deepseek` `zai` `moonshotai` `google` | the six native dialect adapters |
| `openrouter` `vercel_ai_gateway` | fixed-origin gateway adapters with distinct identity/error/receipt handling over the bounded chat core |
| `build_identity` | one compile-time source-tree identity available to external qualification evidence; it is not runtime catalog authority |
| `catalog_port` | bounded signed-collection loading into an immutable content-addressed revision cache, with explicit still-live session-pin coverage |
| `router` | the total eight-authority route, credential affinity/revocation, isolated pool, in-call retry, bounded stream, durable response-start evidence, sealing and receipt |

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
- **Tenant scope is carried per dispatch, never stored as process authority.**
  The immutable ticket supplies organization and workspace. Credential-cache
  and HTTP-pool keys include both plus exact binding revision/generation, and
  each cache uses 16 bounded shards so unrelated brains do not serialize on one
  process-wide mutex. A hot credential hit shares one private zeroizing
  allocation instead of copying plaintext under the shard lock. The KMS branch
  material cache also includes the complete encryption-context digest, so a
  hit cannot bypass exact organization/workspace/name/generation context
  equality.
- **Revocation is a strongest-practical pre-send fence, not atomic with provider
  I/O.** Every send attempt re-reads the exact provider binding, secret metadata,
  and hidden generation concurrently, immediately before socket submission.
  Binding state/revision, secret epoch/state, generation `revoked_at`, ciphertext,
  and context digest must still match. An observed failure invalidates the exact
  decrypted-key and HTTP-pool entries. DynamoDB cannot transact with an external
  provider; a revocation that commits after this read may race with the already
  in-flight attempt, while the next attempt must fail closed.
- **Catalog startup verifies a collection, not merely its newest head.** The
  release supplies an oldest-to-newest contiguous chain plus a sorted unique
  list produced by exact still-live session-retention accounting. Aggregate
  bytes, revision count, envelope bytes, signatures, content addresses, chain
  and signed compatibility policy must all pass before the immutable
  synchronous lookup cache exists. Every declared live pin must be present;
  the loader never guesses a retention window or accepts last-N.
- **Signature verification is all-supplied strict.** An envelope needs at least
  one signature, and every supplied signature must be unique, bounded, trusted
  and valid. A valid trusted signature cannot hide an unknown or bad extra in
  either order. Release rotation is an overlapping, sorted, unique 1–8-key
  compiled trust-root set that can verify old live-pin artifacts and the new
  head together; it is not an any-valid relaxation.
- **The adapter identity is source-derived for assurance evidence.** The
  gateway build script hashes the complete recursively sorted Rust source tree
  with explicit path/content length framing and asserts all eight provider
  modules are in scope. A monitoring receipt may name that digest, but ordinary
  adapter edits do not invalidate the signed compatibility catalog.

### `aex-brain-provider-custody`

This stateless adapter bridges the gateway's narrow credential ports to the
sole workspace-secret ciphertext authority. Resolution performs one
provider-qualified, strongly consistent `pcr_` point read, then reads secret
metadata and the pinned hidden generation concurrently. Revalidation reads all
three exact rows concurrently. Decrypt reconstructs the complete typed
plane/region/organization/workspace/name/generation context, proves its digest
equals the digest stored with the ciphertext, and only then invokes KMS. Tenant
authority comes from the immutable dispatch ticket; no current tenant/default
is stored in `brain-mux`.

### `tests/live/aex-live-model-catalog`

The probe registry (derived from `ProbeId::ALL`, never hand-listed), the
per-provider key resolution, the provider-profile/capability ceiling, the
P-01–P-23 matrix slots, `ReceiptBuilder` and the staging diff remain available
for external assurance. Any future provider-specific monitor must remain
separate from signed compatibility authority and non-gating for builds,
publication, startup, and request admission.

---

## 2. Deferred, with the reason

| Gap | How it is handled |
| --- | --- |
| Provider adapter fixtures | `openai.rs` and `anthropic.rs` both use `fixture::qualified`; adapter unit tests do not carry an offline catalog signature that must be regenerated when the document shape changes. |
| Z.AI's path is recorded two ways in plan 08 §5.4 | The row gives the base as `https://api.z.ai/api/paas/v4` and the path as `POST /paas/v4/chat/completions`, which cannot both be right. The adapter and reviewed compatibility metadata follow the explicit path, producing `https://api.z.ai/paas/v4/chat/completions`; monitoring can detect drift without becoming release authority. |
| `decode`, `finish` and `classify_http` are not handed the `QualifiedModel` | Each adapter therefore compiles its own stop-token and error tables rather than reading `entry.stop_reason_map` / `entry.error_map`. For these eight dialects both are provider-invariant, and `anthropic.rs` asserts the compiled table and the catalog's copy agree. But it means the catalog's copies are documentation for the decode path rather than its source of truth. Widening the trait to take the model would make them authoritative. |
| The 23 probes need a real customer key per provider | The `[[test]]` targets stay undeclared and the manifest keeps `not_applicable.targets` naming OD-07. The harness now exposes one explicit profile and matrix slot per provider/probe, but `PendingProbeExecutor` fails closed until a protected runner performs the calls. Every provider has one authoritative `AEX_LIVE_PROVIDER_KEY_*` name. Anthropic and DeepSeek additionally accept only their documented legacy `ANTHROPIC_API_KEY` and `DEEPSEEK_API_KEY` aliases, after the authoritative name; the other six providers accept no guessed alias. Missing or empty inputs fail loudly, with tests pinning the complete accepted-name list. |
| Additional provider/model compatibility entries | Discovery may propose `Staged` entries, but the reviewed versioned source is the only path to signed `Active` metadata. Provider model-list output never silently changes authority. |
| Provider credential registration | The existing `pcr_` row is a reference to one workspace-secret generation. Exact provider-qualified reads, mutable binding/secret revalidation, and KMS reveal are composed through `aex-brain-provider-custody`; registration remains unmounted because the request has no decided workspace-secret name/collision contract and the active wrapped branch key is not exposed by a port. There is **no** plaintext-from-environment path — not disabled, absent. |
| `resolve_unknown` | Returns `UnknownResolution::NoDurableOperation` for all eight. Implemented, not stubbed: no authority in this set documents a result lookup for a completed streaming generation under AEX's fixed dialects. Anthropic is stateless; OpenAI's `GET /v1/responses/{id}` requires `store: true`, which AEX disables; Gemini Interactions is not the launch dialect. |
| Brain session credential pin | Complete for runtime: `ResolvedAgentConfig` journals a required four-scalar `SessionCredentialPin`, `ProviderPort` requires it, and `DispatchTicket` carries workspace plus organization authority. Revision and generation are non-zero at construction and serde boundaries; epoch zero remains the valid initial epoch. Missing prelaunch pins fail decode; mismatched scope, revision, generation, provider, context digest, binding state, or revocation epoch fails before the next provider send. Session-create admission still has to mint the pin from an explicit credential selection. |
| Installed production catalog binding | `brain-mux` composes a verified immutable collection when the canonical `AEX_MODEL_CATALOG_BINDING_JSON` supplies the exact trust roots, immutable collection URI and both SHA-256 identities. The protected publisher deterministically renders reviewed versioned source, signs only its digest through KMS, verifies genesis or append-only extension and publishes the next candidate. Failed candidates leave the installed last-good binding untouched. |
| Brain content hydration | Canonical requests carry inline user turns. A configured system reference or placed user block fails before dispatch because the application has no content-hydration port yet. |
| `trybuild` type-level leak test | The workspace has no `trybuild` dependency. The same property is asserted by construction — `ProviderApiKey` implements none of `Clone`, `Debug`, `Display`, `Serialize`, `Deref`, and `WireRequest` has no field that can hold one — plus runtime cases over `Debug` output, error bodies and receipts. Adding `trybuild` is a workspace-manifest change and belongs to whoever owns that decision. |
| `miri` over the `credential` module | Not run: the module contains no `unsafe` and the crate forbids it, so `miri` would add build time without a proposition to test. |

### Closing the production catalog authority

The single canonical `AEX_MODEL_CATALOG_BINDING_JSON` Actions variable is an
atomic pointer to already-published signed compatibility metadata, not a way to
create catalog authority. It must remain unset until all of the following exist
together:

1. A dedicated AWS KMS `ECC_NIST_P256` / `SIGN_VERIFY` publisher key and a
   reviewed signing role. Only its uncompressed SEC1 public key enters the
   canonical `aex.model-catalog-trust-roots.v1` document; the private key and
   signing permission never enter public CI.
2. A canonical reviewed `release/model-catalog/*.source.json` document whose
   closed compatibility policy passes the same static loader invariants as the
   runtime. No provider call or live receipt participates in this step.
3. The protected publisher executable prepares the SHA-256 digest of
   `aex-model-catalog/v1\n || document` for KMS `MessageType=DIGEST`, binds the
   closed KMS response, and assembles a canonical genesis or verified
   append-only `aex.model-catalog-collection.v1`. Extension retains all prior
   pins conservatively until a later pruning authority is introduced.
4. The exact collection bytes at the normalized workspace-relative path before
   `artifact plan` runs. The build workflow now downloads only a same-repository
   immutable GitHub release asset and verifies its configured SHA-256 before
   exposing the downloaded workspace-relative collection file to the release
   tool.

Only then may promotion atomically replace `AEX_MODEL_CATALOG_BINDING_JSON`.
The workflow derives the legacy release-tool inputs and local collection path
from that one closed value after download; a runner-specific file path is not a
release binding.
`AEX_TOOL_CATALOG_SHA256` is derived from the checked-out source by the
workflow. The release tool revalidates canonical roots, both digests, the
workspace-relative path, and the built-in tool-catalog digest before compiling
`brain-mux`; startup then verifies the complete signed collection and requires
at least one serviceable `Active` model.

The dedicated signing authority is provisioned in `eu-west-1` and the
protected publisher environment is configured. The remaining bootstrap is to
publish the reviewed source once and install its canonical build binding; no
provider health result is part of that authority.

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
| D-38 | `aex-model-catalog` ships an always-on `fixture` module rather than a feature-gated one | `aex_wire::testing` sets the precedent, and rule 9 of the test architecture forbids test-only build features. `entry()` produces `Staged`; `promote()` is explicit and the static catalog policy gate still validates the resulting signed metadata. |

---

## 5. Changes needed from peers

> **Status correction, 2026-08-01.** Every peer named below has landed, and none of
> these requests was honoured. `aex-brain-domain` did **not** take the
> `aex-model-catalog` dependency; it defines its own `wire_pending` copies of the
> canonical vocabulary and every one of them has since diverged from
> `aex_model_catalog::canonical`. `aex-brain-app` landed `ProviderPort` and
> friends bound to *those* copies, so the gateway's restatement is no longer verbatim
> and the two sides of the port name different request, message, usage and receipt
> types. `aex-brain-app::pressure` is an empty placeholder, so
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
- `TODO(cross-stream): aex-brain-app owns ProviderPort, CatalogPort,
  DispatchTicket, CancelToken, PreviewSink, DispatchProof, DispatchStage,
  DispatchEvidence, EffectIdentity, ProviderOutcome, ProviderDispatchError,
  UnknownResolution, BoxFuture, MemoryReservation and ReservationClass. They are
  restated verbatim in aex-brain-provider-gateway::wire_pending and every one is
  marked for replacement at merge.`
- `TODO(cross-stream): aex-brain-app may widen ProviderFailureClass to the
  fifteen ProviderFailureKind members. Until then ProviderFailureKind::class() is
  the single translation site.`
- `DONE: aex-secret-domain owns SourceGeneration, RevocationEpoch, CiphertextRef
  and EncryptionContext; the gateway imports/re-exports those exact types.`
- `PARTIAL: regional custody owns the pcr_ binding table and secret ciphertext.
  aex-brain-provider-custody implements exact directory reads, revalidation and
  KMS reveal, and brain-mux composes it. Provider registration remains blocked by
  the two explicit contract gaps recorded in §2.`
- `TODO(cross-stream): the contracts stream renders CatalogRevision as
  mc1_<hex of blake3-256> and adds no other catalog field to the public wire.
  aex_wire::CatalogRevision does not exist yet; it lives in
  aex_model_catalog::wire_pending.`
- `TODO(cross-stream): aex-wire's ToolCallId cannot carry a provider-assigned call
  id. If the contracts stream intends it to, it needs a second, bounded-string
  grammar; otherwise Brain re-exports aex_model_catalog::ToolCallId (D-32).`
- `TODO(cross-stream): aex-brain-test-support hosts the provider_fake module this
  stream owns; the peer owns the crate manifest.`
- `PARTIAL: runtimes/brain-mux now composes provider custody/KMS/router and reports
  that binding independently. Signed catalog, tool executors, and Hands backend
  remain named readiness blockers; no wake is received while any remains absent.`

---

## 6. Toolchain note

`aws-lc-sys` does not build on this Windows host without NASM. Every build in
this stream ran with `AWS_LC_SYS_PREBUILT_NASM=1`, which uses the prebuilt
objects shipped in the crate. CI on Linux is unaffected; this is recorded so a
reviewer does not read a local failure as a workspace defect. As plan 08 §12
already notes, `cargo +stable` is also the working local invocation, because the
`rust-toolchain.toml` pin resolves to a rustup toolchain without a `cargo`
component.
