---
title: Provider runtime capabilities
---

# Provider runtime capabilities

Generated from `packages/contracts/src/provider-support.ts` and `packages/contracts/src/models.ts`; runtime routing is derived through `checkRuntimeSupported` and `selectRuntime` in `packages/contracts/src/submission.ts`.

Regenerate with `bun run capabilities:generate`; check with `bun run capabilities:check`.

Providers: [Anthropic](#anthropic) (`anthropic`), [DeepSeek](#deepseek) (`deepseek`), [OpenAI](#openai) (`openai`), [Gemini](#gemini) (`gemini`), [Mistral](#mistral) (`mistral`), [OpenRouter](#openrouter) (`openrouter`), [Doubao](#doubao) (`doubao`), [Doubao (China)](#doubao-cn) (`doubao-cn`). Runtime selectors: `managed`.

All new submissions run on the managed runtime. Public support is expressed as supported providers and supported model ids.

## Supported models

| Provider | Selector | Supported models | Docs | Evidence |
| --- | --- | --- | --- | --- |
| [Anthropic](#anthropic) | `anthropic` | `claude-haiku-4-5`, `claude-3-5-haiku-latest`, `claude-3-5-sonnet-latest`, `claude-sonnet-4-6` | [Secrets](secrets.md); [Events](events.md) | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts); [Installed-SDK Anthropic live user test](../../../apps/user-tests/test/live/live-sdk-anthropic-managed.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts) |
| [DeepSeek](#deepseek) | `deepseek` | `deepseek-v4-flash`, `deepseek-v4-pro`, `deepseek-chat`, `deepseek-reasoner` | [Secrets](secrets.md); [Events](events.md) | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts); [Installed-SDK DeepSeek live user test](../../../apps/user-tests/test/live/live-sdk-deepseek.test.ts); [Installed-SDK DeepSeek comprehensive live user matrix](../../../apps/user-tests/test/live/live-sdk-comprehensive.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts) |
| [OpenAI](#openai) | `openai` | `gpt-4.1`, `gpt-4o-mini` | [Secrets](secrets.md); [Events](events.md) | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts) |
| [Gemini](#gemini) | `gemini` | `gemini-2.0-flash`, `gemini-2.5-flash` | [Secrets](secrets.md); [Events](events.md) | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts) |
| [Mistral](#mistral) | `mistral` | `mistral-large-latest`, `mistral-small-latest` | [Secrets](secrets.md); [Events](events.md) | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts) |
| [OpenRouter](#openrouter) | `openrouter` | `gpt-4o-mini`, `gpt-4o`, `gemini-2.0-flash` | [Secrets](secrets.md); [Events](events.md) | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts) |
| [Doubao](#doubao) | `doubao` | `doubao-seed-pro`, `doubao-seed-flash` | [Secrets](secrets.md); [Events](events.md) | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts) |
| [Doubao (China)](#doubao-cn) | `doubao-cn` | `doubao-seed-pro`, `doubao-seed-flash` | [Secrets](secrets.md); [Events](events.md) | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts) |

## Runtime routing

| Provider | Default provider | Auto route | Runtime selector |
| --- | --- | --- | --- |
| `anthropic` | yes | `managed` | [managed](#anthropic) |
| `deepseek` | no | `managed` | [managed](#deepseek) |
| `openai` | no | `managed` | [managed](#openai) |
| `gemini` | no | `managed` | [managed](#gemini) |
| `mistral` | no | `managed` | [managed](#mistral) |
| `openrouter` | no | `managed` | [managed](#openrouter) |
| `doubao` | no | `managed` | [managed](#doubao) |
| `doubao-cn` | no | `managed` | [managed](#doubao-cn) |

## Runtime evidence

| Provider | Runtime | Enforcement path | Evidence |
| --- | --- | --- | --- |
| `anthropic` | `managed` | submission parser + managed dispatch | [Installed-SDK Anthropic live user test](../../../apps/user-tests/test/live/live-sdk-anthropic-managed.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts) |
| `deepseek` | `managed` | submission parser + managed dispatch | [Installed-SDK DeepSeek live user test](../../../apps/user-tests/test/live/live-sdk-deepseek.test.ts); [Installed-SDK DeepSeek comprehensive live user matrix](../../../apps/user-tests/test/live/live-sdk-comprehensive.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts) |
| `openai` | `managed` | submission parser + managed dispatch | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts) |
| `gemini` | `managed` | submission parser + managed dispatch | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts) |
| `mistral` | `managed` | submission parser + managed dispatch | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts) |
| `openrouter` | `managed` | submission parser + managed dispatch | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts) |
| `doubao` | `managed` | submission parser + managed dispatch | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts) |
| `doubao-cn` | `managed` | submission parser + managed dispatch | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts) |

## Validation errors

| Code | Docs anchor | Enforcement path | Evidence |
| --- | --- | --- | --- |
| `feature_runtime_mismatch` | [managed-unsupported-features](#managed-unsupported-features) | collectManagedUnsupportedFeatures + selectRuntime | [Submission parser and routing parity](../../contracts/test/submission.test.ts) |

### Managed unsupported features

Provider-hosted skill refs (a `kind:"provider"` skill ref) are rejected because new runs dispatch to the managed runtime. Supply skill bytes through `Skill.fromFiles`, `Skill.fromPath`, `Skill.fromUrl`, or `Skill.fromCatalog`; each path normalizes to an asset that the platform snapshots into durable run asset storage.

Notes:

- Supported models are the public SDK model ids accepted for each provider.
- Runtime routing describes how a validated submission is dispatched.
- `runtime: "native"` is not a runtime selector; the submission parser rejects it as an invalid enum value.

## Provider anchors

### Anthropic

- Wire provider: `anthropic`
- Supported models: `claude-haiku-4-5`, `claude-3-5-haiku-latest`, `claude-3-5-sonnet-latest`, `claude-sonnet-4-6`
- Auto route: `managed`
- Docs: [Secrets](secrets.md); [Events](events.md)
- Evidence: [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts); [Installed-SDK Anthropic live user test](../../../apps/user-tests/test/live/live-sdk-anthropic-managed.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts)

### DeepSeek

- Wire provider: `deepseek`
- Supported models: `deepseek-v4-flash`, `deepseek-v4-pro`, `deepseek-chat`, `deepseek-reasoner`
- Auto route: `managed`
- Docs: [Secrets](secrets.md); [Events](events.md)
- Evidence: [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts); [Installed-SDK DeepSeek live user test](../../../apps/user-tests/test/live/live-sdk-deepseek.test.ts); [Installed-SDK DeepSeek comprehensive live user matrix](../../../apps/user-tests/test/live/live-sdk-comprehensive.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts)

### OpenAI

- Wire provider: `openai`
- Supported models: `gpt-4.1`, `gpt-4o-mini`
- Auto route: `managed`
- Docs: [Secrets](secrets.md); [Events](events.md)
- Evidence: [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts)

### Gemini

- Wire provider: `gemini`
- Supported models: `gemini-2.0-flash`, `gemini-2.5-flash`
- Auto route: `managed`
- Docs: [Secrets](secrets.md); [Events](events.md)
- Evidence: [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts)

### Mistral

- Wire provider: `mistral`
- Supported models: `mistral-large-latest`, `mistral-small-latest`
- Auto route: `managed`
- Docs: [Secrets](secrets.md); [Events](events.md)
- Evidence: [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts)

### OpenRouter

- Wire provider: `openrouter`
- Supported models: `gpt-4o-mini`, `gpt-4o`, `gemini-2.0-flash`
- Auto route: `managed`
- Docs: [Secrets](secrets.md); [Events](events.md)
- Evidence: [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts)

### Doubao

- Wire provider: `doubao`
- Supported models: `doubao-seed-pro`, `doubao-seed-flash`
- Auto route: `managed`
- Docs: [Secrets](secrets.md); [Events](events.md)
- Evidence: [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts)

### Doubao (China)

- Wire provider: `doubao-cn`
- Supported models: `doubao-seed-pro`, `doubao-seed-flash`
- Auto route: `managed`
- Docs: [Secrets](secrets.md); [Events](events.md)
- Evidence: [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts)
