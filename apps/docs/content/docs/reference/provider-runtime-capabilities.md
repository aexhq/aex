---
title: Provider runtime capabilities
---

# Provider runtime capabilities

Generated from `packages/contracts/src/provider-support.ts` and `packages/contracts/src/models.ts`.

Regenerate with `bun run capabilities:generate`; check with `bun run capabilities:check`.

Providers: [Anthropic](#anthropic) (`anthropic`), [DeepSeek](#deepseek) (`deepseek`), [OpenAI](#openai) (`openai`), [Gemini](#gemini) (`gemini`), [Mistral](#mistral) (`mistral`), [OpenRouter](#openrouter) (`openrouter`), [Doubao](#doubao) (`doubao`), [Doubao (China)](#doubao-cn) (`doubao-cn`).

All new submissions run on the managed runtime. Public support is expressed as supported providers and supported model ids.

## Supported models

| Provider | Selector | Supported models | Docs | Evidence |
| --- | --- | --- | --- | --- |
| [Anthropic](#anthropic) | `anthropic` | `claude-haiku-4-5`, `claude-3-5-haiku-latest`, `claude-3-5-sonnet-latest`, `claude-sonnet-4-6` | [Secrets](/docs/guides/secrets/); [Events](/docs/guides/events/) | [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts); [Installed-SDK Anthropic live user test](https://github.com/aexhq/aex/blob/main/apps/user-tests/test/live/live-sdk-anthropic-managed.test.ts) |
| [DeepSeek](#deepseek) | `deepseek` | `deepseek-v4-flash`, `deepseek-v4-pro` | [Secrets](/docs/guides/secrets/); [Events](/docs/guides/events/) | [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts); [Installed-SDK DeepSeek live user test](https://github.com/aexhq/aex/blob/main/apps/user-tests/test/live/live-sdk-deepseek.test.ts); [Installed-SDK DeepSeek comprehensive live user matrix](https://github.com/aexhq/aex/blob/main/apps/user-tests/test/live/live-sdk-comprehensive.test.ts) |
| [OpenAI](#openai) | `openai` | `gpt-4.1`, `gpt-4o-mini` | [Secrets](/docs/guides/secrets/); [Events](/docs/guides/events/) | [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts) |
| [Gemini](#gemini) | `gemini` | `gemini-2.0-flash`, `gemini-2.5-flash` | [Secrets](/docs/guides/secrets/); [Events](/docs/guides/events/) | [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts) |
| [Mistral](#mistral) | `mistral` | `mistral-large-latest`, `mistral-small-latest` | [Secrets](/docs/guides/secrets/); [Events](/docs/guides/events/) | [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts) |
| [OpenRouter](#openrouter) | `openrouter` | `gpt-4o-mini`, `gpt-4o`, `gemini-2.0-flash` | [Secrets](/docs/guides/secrets/); [Events](/docs/guides/events/) | [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts) |
| [Doubao](#doubao) | `doubao` | `doubao-seed-pro`, `doubao-seed-flash` | [Secrets](/docs/guides/secrets/); [Events](/docs/guides/events/) | [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts) |
| [Doubao (China)](#doubao-cn) | `doubao-cn` | `doubao-seed-pro`, `doubao-seed-flash` | [Secrets](/docs/guides/secrets/); [Events](/docs/guides/events/) | [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts) |

## Managed evidence

| Provider | Enforcement path | Evidence |
| --- | --- | --- |
| `anthropic` | submission parser + managed execution | [Installed-SDK Anthropic live user test](https://github.com/aexhq/aex/blob/main/apps/user-tests/test/live/live-sdk-anthropic-managed.test.ts) |
| `deepseek` | submission parser + managed execution | [Installed-SDK DeepSeek live user test](https://github.com/aexhq/aex/blob/main/apps/user-tests/test/live/live-sdk-deepseek.test.ts); [Installed-SDK DeepSeek comprehensive live user matrix](https://github.com/aexhq/aex/blob/main/apps/user-tests/test/live/live-sdk-comprehensive.test.ts) |
| `openai` | submission parser + managed execution | [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts) |
| `gemini` | submission parser + managed execution | [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts) |
| `mistral` | submission parser + managed execution | [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts) |
| `openrouter` | submission parser + managed execution | [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts) |
| `doubao` | submission parser + managed execution | [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts) |
| `doubao-cn` | submission parser + managed execution | [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts) |

## Skills

Only asset-backed skills are accepted on submissions. Supply skill bytes through `Skill.fromFiles`, `Skill.fromPath`, `Skill.fromUrl`, or `Skill.fromCatalog`; each path normalizes to an asset that the platform snapshots into durable run asset storage.

Notes:

- Supported models are the public SDK model ids accepted for each provider.
- Execution uses the managed path; there is no public runtime selector.

## Provider anchors

### Anthropic

- Wire provider: `anthropic`
- Supported models: `claude-haiku-4-5`, `claude-3-5-haiku-latest`, `claude-3-5-sonnet-latest`, `claude-sonnet-4-6`
- Docs: [Secrets](/docs/guides/secrets/); [Events](/docs/guides/events/)
- Evidence: [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts); [Installed-SDK Anthropic live user test](https://github.com/aexhq/aex/blob/main/apps/user-tests/test/live/live-sdk-anthropic-managed.test.ts)

### DeepSeek

- Wire provider: `deepseek`
- Supported models: `deepseek-v4-flash`, `deepseek-v4-pro`
- Docs: [Secrets](/docs/guides/secrets/); [Events](/docs/guides/events/)
- Evidence: [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts); [Installed-SDK DeepSeek live user test](https://github.com/aexhq/aex/blob/main/apps/user-tests/test/live/live-sdk-deepseek.test.ts); [Installed-SDK DeepSeek comprehensive live user matrix](https://github.com/aexhq/aex/blob/main/apps/user-tests/test/live/live-sdk-comprehensive.test.ts)

### OpenAI

- Wire provider: `openai`
- Supported models: `gpt-4.1`, `gpt-4o-mini`
- Docs: [Secrets](/docs/guides/secrets/); [Events](/docs/guides/events/)
- Evidence: [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts)

### Gemini

- Wire provider: `gemini`
- Supported models: `gemini-2.0-flash`, `gemini-2.5-flash`
- Docs: [Secrets](/docs/guides/secrets/); [Events](/docs/guides/events/)
- Evidence: [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts)

### Mistral

- Wire provider: `mistral`
- Supported models: `mistral-large-latest`, `mistral-small-latest`
- Docs: [Secrets](/docs/guides/secrets/); [Events](/docs/guides/events/)
- Evidence: [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts)

### OpenRouter

- Wire provider: `openrouter`
- Supported models: `gpt-4o-mini`, `gpt-4o`, `gemini-2.0-flash`
- Docs: [Secrets](/docs/guides/secrets/); [Events](/docs/guides/events/)
- Evidence: [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts)

### Doubao

- Wire provider: `doubao`
- Supported models: `doubao-seed-pro`, `doubao-seed-flash`
- Docs: [Secrets](/docs/guides/secrets/); [Events](/docs/guides/events/)
- Evidence: [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts)

### Doubao (China)

- Wire provider: `doubao-cn`
- Supported models: `doubao-seed-pro`, `doubao-seed-flash`
- Docs: [Secrets](/docs/guides/secrets/); [Events](/docs/guides/events/)
- Evidence: [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts)
