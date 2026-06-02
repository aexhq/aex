---
title: Provider runtime capabilities
---

# Provider runtime capabilities

Generated from `packages/contracts/src/provider-support.ts` and `packages/contracts/src/provider-capability.ts`; runtime cells are derived through `checkRuntimeSupported` and `selectRuntime` in `packages/contracts/src/submission.ts`.

Regenerate with `pnpm capabilities:generate`; check with `pnpm capabilities:check`.

Providers: [Anthropic](#anthropic) (`anthropic`), [DeepSeek](#deepseek) (`deepseek`), [OpenAI](#openai) (`openai`), [Gemini](#gemini) (`gemini`), [Mistral](#mistral) (`mistral`). Runtime selectors: `native`, `managed`.

Public support facts are listed separately from runtime routing facts. Goose Managed is the universal managed runtime. A provider-native runtime is used only when the shared capability registry declares one.

Status vocabulary: `supported`, `live-unverified`, `provider-inherited`, `rejected`.

## Public support

| Provider | Wire value | Status | Docs | Evidence |
| --- | --- | --- | --- | --- |
| [Anthropic](#anthropic) | `anthropic` | supported | [Credentials](credentials.md); [Events](events.md) | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts); [Native feature parity gate](../../contracts/test/native-feature-gate.test.ts) |
| [DeepSeek](#deepseek) | `deepseek` | supported | [Credentials](credentials.md); [Events](events.md) | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts); [Installed-SDK live user matrix](../../../apps/user-tests/test/live/live-sdk-comprehensive.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts) |
| [OpenAI](#openai) | `openai` | live-unverified | [Credentials](credentials.md); [Events](events.md) | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts) |
| [Gemini](#gemini) | `gemini` | live-unverified | [Credentials](credentials.md); [Events](events.md) | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts) |
| [Mistral](#mistral) | `mistral` | live-unverified | [Credentials](credentials.md); [Events](events.md) | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts) |

## Runtime routing

| Provider | Default provider | Auto route | `runtime: "native"` | `runtime: "managed"` | Native executor |
| --- | --- | --- | --- | --- | --- |
| `anthropic` | yes | `native` | [supported](#anthropic); provider-inherited | [supported](#anthropic) | `anthropic-managed` |
| `deepseek` | no | `managed` | [rejected](#deepseek) | [supported](#deepseek) | n/a |
| `openai` | no | `managed` | [rejected](#openai) | [live-unverified](#openai) | n/a |
| `gemini` | no | `managed` | [rejected](#gemini) | [live-unverified](#gemini) | n/a |
| `mistral` | no | `managed` | [rejected](#mistral) | [live-unverified](#mistral) | n/a |

## Runtime cell evidence

| Provider | Runtime | Status | Ownership | Enforcement path | Evidence |
| --- | --- | --- | --- | --- | --- |
| `anthropic` | `native` | supported | provider-inherited | submission parser + provider-native dispatch | [Installed-SDK live user matrix](../../../apps/user-tests/test/live/live-sdk-comprehensive.test.ts); [Native feature parity gate](../../contracts/test/native-feature-gate.test.ts) |
| `anthropic` | `managed` | supported | supported | submission parser + Goose Managed dispatch | [Installed-SDK live user matrix](../../../apps/user-tests/test/live/live-sdk-comprehensive.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts) |
| `deepseek` | `native` | rejected | rejected | checkRuntimeSupported runtime_native_unsupported | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts); [Installed-SDK live user matrix](../../../apps/user-tests/test/live/live-sdk-comprehensive.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts) |
| `deepseek` | `managed` | supported | supported | submission parser + Goose Managed dispatch | [Installed-SDK live user matrix](../../../apps/user-tests/test/live/live-sdk-comprehensive.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts) |
| `openai` | `native` | rejected | rejected | checkRuntimeSupported runtime_native_unsupported | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts) |
| `openai` | `managed` | live-unverified | live-unverified | submission parser + Goose Managed dispatch | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts) |
| `gemini` | `native` | rejected | rejected | checkRuntimeSupported runtime_native_unsupported | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts) |
| `gemini` | `managed` | live-unverified | live-unverified | submission parser + Goose Managed dispatch | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts) |
| `mistral` | `native` | rejected | rejected | checkRuntimeSupported runtime_native_unsupported | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts) |
| `mistral` | `managed` | live-unverified | live-unverified | submission parser + Goose Managed dispatch | [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts) |

## Native feature parity

| Provider | Native inline skills | Native files | Native MCP servers |
| --- | --- | --- | --- |
| `anthropic` | supported | supported | supported |
| `deepseek` | n/a | n/a | n/a |
| `openai` | n/a | n/a | n/a |
| `gemini` | n/a | n/a | n/a |
| `mistral` | n/a | n/a | n/a |

Notes:

- Public status describes provider availability on the SDK surface. Runtime routing describes how a validated submission is dispatched.
- `rejected` for `runtime: "native"` means the submission parser returns `runtime_native_unsupported` for that provider.
- `live-unverified` means the shape is accepted by code but lacks equal live user evidence in this repository.
- `provider-inherited` means antpath passes a capability through while the provider or customer-controlled service owns final behavior.
- Native feature parity cells come from `PROVIDER_CAPABILITY[provider].nativeAgent.serves`; `n/a` means the provider has no native agent runtime.

## Provider anchors

### Anthropic

- Wire provider: `anthropic`
- Public status: supported
- Auto route: `native`
- Docs: [Credentials](credentials.md); [Events](events.md)
- Evidence: [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts); [Native feature parity gate](../../contracts/test/native-feature-gate.test.ts)

### DeepSeek

- Wire provider: `deepseek`
- Public status: supported
- Auto route: `managed`
- Docs: [Credentials](credentials.md); [Events](events.md)
- Evidence: [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts); [Installed-SDK live user matrix](../../../apps/user-tests/test/live/live-sdk-comprehensive.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts)

### OpenAI

- Wire provider: `openai`
- Public status: live-unverified
- Auto route: `managed`
- Docs: [Credentials](credentials.md); [Events](events.md)
- Evidence: [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts)

### Gemini

- Wire provider: `gemini`
- Public status: live-unverified
- Auto route: `managed`
- Docs: [Credentials](credentials.md); [Events](events.md)
- Evidence: [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts)

### Mistral

- Wire provider: `mistral`
- Public status: live-unverified
- Auto route: `managed`
- Docs: [Credentials](credentials.md); [Events](events.md)
- Evidence: [Submission parser and routing parity](../../contracts/test/submission.test.ts); [Runtime support validator](../../contracts/test/runtime-support.test.ts); [Generated matrix freshness](../../../scripts/validate/capability-matrix.test.ts)
