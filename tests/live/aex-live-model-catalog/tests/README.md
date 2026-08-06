# `aex-live-model-catalog` live tests

Integration tests for the `model-catalog` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: the real provider model qualification matrix plus disable and change probes.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.

## Qualification readiness

`aex_live_model_catalog::executor` is the closed readiness boundary for the
future protected live runner. It does not select a model from a provider model
list and it does not mint a receipt: every run supplies the exact model slug
and capability set that it is about to qualify. The profile rejects capability
bits outside the corresponding adapter's declared ceiling, then builds one
slot for every `P-01` through `P-23` probe. Capability-gated probes become
explicit `NotApplicable` rows only when that capability is not declared;
unconditional probes are always required.

Credential names are explicit and provider-affine:

| Provider authority | Authoritative variable | Accepted legacy alias |
| --- | --- | --- |
| `openai` | `AEX_LIVE_PROVIDER_KEY_OPENAI` | none |
| `anthropic` | `AEX_LIVE_PROVIDER_KEY_ANTHROPIC` | `ANTHROPIC_API_KEY` |
| `deepseek` | `AEX_LIVE_PROVIDER_KEY_DEEPSEEK` | `DEEPSEEK_API_KEY` |
| `zai` | `AEX_LIVE_PROVIDER_KEY_ZAI` | none |
| `moonshotai` | `AEX_LIVE_PROVIDER_KEY_MOONSHOTAI` | none |
| `google` | `AEX_LIVE_PROVIDER_KEY_GOOGLE` | none |
| `openrouter` | `AEX_LIVE_PROVIDER_KEY_OPENROUTER` | none |
| `vercel_ai_gateway` | `AEX_LIVE_PROVIDER_KEY_VERCEL_AI_GATEWAY` | none |

The authoritative name wins when both names exist; an empty or non-Unicode
value fails, and no key value is retained in the matrix. The generic
`PendingProbeExecutor` intentionally fails on the first required probe, so it
cannot be mistaken for live evidence.

`ConfiguredProbeExecutor` is the reusable next boundary: it runs
asynchronously over one exact target and one explicit owner-supplied program
per probe. Programs return closed typed evidence rather than a verdict; the
shared P-01--P-23 verifier is the only code that can produce `Pass`. Missing,
duplicate and cross-target programs, plus evidence for the wrong probe, all
fail closed. Driver failures may carry only the gateway's bounded
`RedactedDetail`, and receipt facts are made from closed enums, hashes and
counters rather than provider response strings.

The `aex-model-catalog-qualifier` executable and
`model-catalog-qualify.yml` protected workflow now bind the first exact target,
`deepseek/deepseek-v4-flash`. `preflight` reports one production program for
every P-01 through P-23 probe. Completeness and all fixed identities are checked
before environment credential acquisition, HTTP-client creation, or provider
I/O. A failed run emits neither evidence nor a receipt. The qualifier cannot
emit a `CatalogDocument`; static catalog source and protected publication are a
separate release path.

Local canned outcomes remain explicitly rejected as evidence. P-15 consumes a
real provider response through the production stream core, while P-18 uses the
production send gate for its pre-header boundary. The post-head drop,
oversized-frame and idle schedules run through the same bounded production
stream consumer used by the router without making billable throwaway calls;
their evidence is derived from its typed failure, dispatch proof, counters and
elapsed timer. P-20 uses the
production transport and DeepSeek HTTP classifier with redacted response shapes. The public DeepSeek adapter also
exposes a qualification-only request seam for a reviewed `Staged` entry; it
reuses the private production request builder without accepting a credential,
origin override, provider default, or already-qualified model.

The protected owner must still supply all of the following before preflight can
admit a live run:

- the exact reviewed provider/model pair, capabilities and full compatibility
  `ModelEntry`; adapter test fixtures and arbitrary model strings are not
  qualification inputs;
- the pinned official tokenizer input used to verify the exact P-16 and P-17
  context corpora;
- the customer-owned key through the protected runner's approved custody path,
  using only `AEX_LIVE_PROVIDER_KEY_DEEPSEEK`; the protected wrapper zeroizes
  owned bytes and exposes only redacted diagnostics;
- exact per-probe canonical request programs and deterministic oracles:
  prompts, tool schemas/results, structured-output schema, reasoning replay,
  cache prefix, stop sequence, tokenizer/context corpus, and output bounds;
- an owner-approved negative-test credential procedure and guaranteed-unknown
  model input for P-20; neither value may be guessed or committed;
- a bounded transport/fault harness capable of non-stream parity, cancellation,
  the four P-18 drop points, oversized-frame injection, idle injection, exact
  byte/frame accounting, and the production adapter's diagnostic redaction;
- receipt metadata and custody for the redacted evidence bundle, request ids,
  token totals, evidence digest, plane/region/time and adapter-source digest.

If any required authority or live assertion is unavailable, execution stops
and no evidence or receipt is emitted. This monitoring result does not create,
remove, or block publication of repository-reviewed signed catalog metadata.
