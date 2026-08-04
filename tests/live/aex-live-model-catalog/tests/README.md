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
