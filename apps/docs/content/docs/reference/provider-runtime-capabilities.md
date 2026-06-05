---
title: Provider runtime capabilities
---

# Provider runtime capabilities

Generated from `packages/contracts/src/provider-support.ts`; runtime cells are derived through `checkRuntimeSupported` and `selectRuntime` in `packages/contracts/src/submission.ts`.

Regenerate with `pnpm capabilities:generate`; check with `pnpm capabilities:check`.

Providers: [Anthropic](#anthropic) (`anthropic`), [DeepSeek](#deepseek) (`deepseek`), [OpenAI](#openai) (`openai`), [Gemini](#gemini) (`gemini`), [Mistral](#mistral) (`mistral`). Runtime selectors: `managed`.

All new submissions run on the managed runtime. Public support facts are listed separately from runtime dispatch facts.

Status vocabulary: `supported`, `live-unverified`, `rejected`.

## Public support

| Provider | Wire value | Status | Docs | Evidence |
| --- | --- | --- | --- | --- |
| [Anthropic](#anthropic) | `anthropic` | supported | [Credentials](/docs/guides/credentials/); [Events](/docs/guides/events/) | [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Runtime support validator](https://github.com/aexhq/aex/blob/main/packages/contracts/test/runtime-support.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts); [Installed-SDK live user matrix](https://github.com/aexhq/aex/blob/main/apps/user-tests/test/live/live-sdk-comprehensive.test.ts); [Runtime support validator](https://github.com/aexhq/aex/blob/main/packages/contracts/test/runtime-support.test.ts) |
| [DeepSeek](#deepseek) | `deepseek` | supported | [Credentials](/docs/guides/credentials/); [Events](/docs/guides/events/) | [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Runtime support validator](https://github.com/aexhq/aex/blob/main/packages/contracts/test/runtime-support.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts); [Installed-SDK live user matrix](https://github.com/aexhq/aex/blob/main/apps/user-tests/test/live/live-sdk-comprehensive.test.ts); [Runtime support validator](https://github.com/aexhq/aex/blob/main/packages/contracts/test/runtime-support.test.ts) |
| [OpenAI](#openai) | `openai` | live-unverified | [Credentials](/docs/guides/credentials/); [Events](/docs/guides/events/) | [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Runtime support validator](https://github.com/aexhq/aex/blob/main/packages/contracts/test/runtime-support.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts) |
| [Gemini](#gemini) | `gemini` | live-unverified | [Credentials](/docs/guides/credentials/); [Events](/docs/guides/events/) | [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Runtime support validator](https://github.com/aexhq/aex/blob/main/packages/contracts/test/runtime-support.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts) |
| [Mistral](#mistral) | `mistral` | live-unverified | [Credentials](/docs/guides/credentials/); [Events](/docs/guides/events/) | [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Runtime support validator](https://github.com/aexhq/aex/blob/main/packages/contracts/test/runtime-support.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts) |

## Runtime routing

| Provider | Default provider | Auto route | `runtime: "managed"` |
| --- | --- | --- | --- |
| `anthropic` | yes | `managed` | [supported](#anthropic) |
| `deepseek` | no | `managed` | [supported](#deepseek) |
| `openai` | no | `managed` | [live-unverified](#openai) |
| `gemini` | no | `managed` | [live-unverified](#gemini) |
| `mistral` | no | `managed` | [live-unverified](#mistral) |

## Runtime cell evidence

| Provider | Runtime | Status | Ownership | Enforcement path | Evidence |
| --- | --- | --- | --- | --- | --- |
| `anthropic` | `managed` | supported | supported | submission parser + managed dispatch | [Installed-SDK live user matrix](https://github.com/aexhq/aex/blob/main/apps/user-tests/test/live/live-sdk-comprehensive.test.ts); [Runtime support validator](https://github.com/aexhq/aex/blob/main/packages/contracts/test/runtime-support.test.ts) |
| `deepseek` | `managed` | supported | supported | submission parser + managed dispatch | [Installed-SDK live user matrix](https://github.com/aexhq/aex/blob/main/apps/user-tests/test/live/live-sdk-comprehensive.test.ts); [Runtime support validator](https://github.com/aexhq/aex/blob/main/packages/contracts/test/runtime-support.test.ts) |
| `openai` | `managed` | live-unverified | live-unverified | submission parser + managed dispatch | [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Runtime support validator](https://github.com/aexhq/aex/blob/main/packages/contracts/test/runtime-support.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts) |
| `gemini` | `managed` | live-unverified | live-unverified | submission parser + managed dispatch | [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Runtime support validator](https://github.com/aexhq/aex/blob/main/packages/contracts/test/runtime-support.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts) |
| `mistral` | `managed` | live-unverified | live-unverified | submission parser + managed dispatch | [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Runtime support validator](https://github.com/aexhq/aex/blob/main/packages/contracts/test/runtime-support.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts) |

## Validation errors

| Code | Docs anchor | Enforcement path | Evidence |
| --- | --- | --- | --- |
| `feature_runtime_mismatch` | [managed-unsupported-features](#managed-unsupported-features) | collectManagedUnsupportedFeatures + selectRuntime | [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts) |

### Managed unsupported features

Provider-hosted skill refs such as `Skill.provider(...)` are rejected because new runs dispatch to the managed runtime. Use inline aex skills or remove the provider-hosted ref.

Notes:

- Public status describes provider availability on the SDK surface. Runtime routing describes how a validated submission is dispatched.
- `runtime: "native"` is not a runtime selector; the submission parser rejects it as an invalid enum value.
- `live-unverified` means the shape is accepted by code but does not yet have equal live user-test evidence.

## Provider anchors

### Anthropic

- Wire provider: `anthropic`
- Public status: supported
- Auto route: `managed`
- Docs: [Credentials](/docs/guides/credentials/); [Events](/docs/guides/events/)
- Evidence: [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Runtime support validator](https://github.com/aexhq/aex/blob/main/packages/contracts/test/runtime-support.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts); [Installed-SDK live user matrix](https://github.com/aexhq/aex/blob/main/apps/user-tests/test/live/live-sdk-comprehensive.test.ts); [Runtime support validator](https://github.com/aexhq/aex/blob/main/packages/contracts/test/runtime-support.test.ts)

### DeepSeek

- Wire provider: `deepseek`
- Public status: supported
- Auto route: `managed`
- Docs: [Credentials](/docs/guides/credentials/); [Events](/docs/guides/events/)
- Evidence: [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Runtime support validator](https://github.com/aexhq/aex/blob/main/packages/contracts/test/runtime-support.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts); [Installed-SDK live user matrix](https://github.com/aexhq/aex/blob/main/apps/user-tests/test/live/live-sdk-comprehensive.test.ts); [Runtime support validator](https://github.com/aexhq/aex/blob/main/packages/contracts/test/runtime-support.test.ts)

### OpenAI

- Wire provider: `openai`
- Public status: live-unverified
- Auto route: `managed`
- Docs: [Credentials](/docs/guides/credentials/); [Events](/docs/guides/events/)
- Evidence: [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Runtime support validator](https://github.com/aexhq/aex/blob/main/packages/contracts/test/runtime-support.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts)

### Gemini

- Wire provider: `gemini`
- Public status: live-unverified
- Auto route: `managed`
- Docs: [Credentials](/docs/guides/credentials/); [Events](/docs/guides/events/)
- Evidence: [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Runtime support validator](https://github.com/aexhq/aex/blob/main/packages/contracts/test/runtime-support.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts)

### Mistral

- Wire provider: `mistral`
- Public status: live-unverified
- Auto route: `managed`
- Docs: [Credentials](/docs/guides/credentials/); [Events](/docs/guides/events/)
- Evidence: [Submission parser and routing parity](https://github.com/aexhq/aex/blob/main/packages/contracts/test/submission.test.ts); [Runtime support validator](https://github.com/aexhq/aex/blob/main/packages/contracts/test/runtime-support.test.ts); [Generated matrix freshness](https://github.com/aexhq/aex/blob/main/scripts/validate/capability-matrix.test.ts)
