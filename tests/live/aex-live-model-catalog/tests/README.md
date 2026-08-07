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
value fails, and no key value is retained in the matrix. The protected runner
must obtain the secret through the approved custody path before making a real
provider call. `PendingProbeExecutor` intentionally fails on the first
required probe, so this scaffolding cannot be mistaken for live evidence.

`ConfiguredProbeExecutor` is the reusable next boundary: it runs
asynchronously over one exact target and one explicit owner-supplied program
per probe. Programs return closed typed evidence rather than a verdict; the
shared P-01--P-23 verifier is the only code that can produce `Pass`. Missing,
duplicate and cross-target programs, plus evidence for the wrong probe, all
fail closed. Driver failures may carry only the gateway's bounded
`RedactedDetail`, and receipt facts are made from closed enums, hashes and
counters rather than provider response strings.

No executable live profile is checked in yet. The protected owner must supply
all of the following before replacing `PendingProbeExecutor` in a live target:

- the exact reviewed provider/model pair, capabilities, full `ModelEntry` and
  signed catalog revision; adapter test fixtures and syntactically valid model
  strings are not catalog authority;
- the customer-owned key through the protected runner's approved custody path,
  using the accepted variable identity above; the executor never reads an
  environment value or stores plaintext;
- exact per-probe canonical request programs and deterministic oracles:
  prompts, tool schemas/results, structured-output schema, reasoning replay,
  cache prefix, stop sequence, tokenizer/context corpus, and output bounds;
- an owner-approved negative-test credential procedure and guaranteed-unknown
  model input for P-20; neither value may be guessed or committed;
- a bounded transport/fault harness capable of non-stream parity, cancellation,
  the four P-18 drop points, oversized-frame injection, idle injection, exact
  byte/frame accounting, and the production adapter's diagnostic redaction;
- receipt metadata and custody for the redacted evidence bundle, request ids,
  token totals, evidence digest, plane/region/time, adapter-source digest and
  protected publisher signature.

Until every required program and authority above exists, execution stops at
the first unavailable slot and no receipt or catalog asset is emitted.
