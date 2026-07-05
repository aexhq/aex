import type { RunProvider } from "./submission.js";
import { suggest } from "./suggest.js";

/**
 * Source of truth for the closed model set: each canonical model id maps to the
 * upstream providers that can serve it and the **provider-native** model string
 * each one expects.
 *
 * `Models.*` / `RUN_MODELS` are aex's own **canonical, provider-neutral**
 * identifiers — they are NOT the strings sent to a provider. The platform
 * translates a `(canonical model, provider)` pair to the native id via
 * {@link resolveProviderModelId} when it builds the run's session manifest. The
 * same canonical model can therefore be served by more than one provider (e.g.
 * `gpt-4o-mini` via `openai` *or* `openrouter`), with a different native string
 * per provider.
 *
 * Ordering matters: the **first** provider listed for a model is its default
 * (see {@link providerForModel}) — list the native vendor before `openrouter`.
 * Additions belong here first so SDK types, CLI validation, docs examples, and
 * platform parsing all move together.
 */
export const MODEL_PROVIDER_IDS = {
  "claude-haiku-4-5": { anthropic: "claude-haiku-4-5" },
  "claude-3-5-haiku-latest": { anthropic: "claude-3-5-haiku-latest" },
  "claude-3-5-sonnet-latest": { anthropic: "claude-3-5-sonnet-latest" },
  "claude-sonnet-4-6": { anthropic: "claude-sonnet-4-6" },
  "deepseek-v4-flash": { deepseek: "deepseek-v4-flash" },
  "deepseek-v4-pro": { deepseek: "deepseek-v4-pro" },
  "gpt-4.1": { openai: "gpt-4.1" },
  "gpt-4o-mini": { openai: "gpt-4o-mini", openrouter: "openai/gpt-4o-mini" },
  "gpt-4o": { openrouter: "openai/gpt-4o" },
  "gemini-2.0-flash": { gemini: "gemini-2.0-flash", openrouter: "google/gemini-2.0-flash-001" },
  "gemini-2.5-flash": { gemini: "gemini-2.5-flash" },
  "mistral-large-latest": { mistral: "mistral-large-latest" },
  "mistral-small-latest": { mistral: "mistral-small-latest" },
  // Doubao (ByteDance) via the official Ark API. Served by both the
  // international BytePlus ModelArk gateway (`doubao`, the default) and the
  // China Volcengine Ark gateway (`doubao-cn`). Ark accepts the API-format
  // model NAME directly in the chat-completions `model` field (no `ep-…`
  // inference-endpoint id). The native strings are the same Ark catalog ids on
  // both gateways.
  //   pro   — Doubao Seed 1.8 (flagship, 256K context).
  //   flash — Doubao Seed 1.6 Flash (fast/cheap, 256K context).
  "doubao-seed-pro": { doubao: "doubao-seed-1-8-251228", "doubao-cn": "doubao-seed-1-8-251228" },
  "doubao-seed-flash": {
    doubao: "doubao-seed-1-6-flash-250828",
    "doubao-cn": "doubao-seed-1-6-flash-250828"
  }
} as const satisfies Readonly<Record<string, Partial<Record<RunProvider, string>>>>;

/**
 * Closed set of canonical model ids accepted by the public run-submission
 * schema. Derived from {@link MODEL_PROVIDER_IDS} so the two never drift.
 */
export type RunModel = keyof typeof MODEL_PROVIDER_IDS;

export const RUN_MODELS = Object.keys(MODEL_PROVIDER_IDS) as readonly RunModel[];

/**
 * Symbol-style accessors for the closed model set. Prefer these over raw
 * strings so an invalid token is a compile error, not a runtime 400 — e.g.
 * `Models.CLAUDE_HAIKU_4_5`. These are aex's **canonical** ids, not the native
 * strings sent upstream; the platform translates them per provider (see
 * {@link MODEL_PROVIDER_IDS} / {@link resolveProviderModelId}). When a model is
 * served by more than one provider, pair it with an explicit {@link Providers}
 * value; otherwise the single (default) provider is used.
 */
export const Models = {
  /** Claude Haiku 4.5 — Anthropic. */
  CLAUDE_HAIKU_4_5: "claude-haiku-4-5",
  /** Claude 3.5 Haiku (latest) — Anthropic. */
  CLAUDE_3_5_HAIKU_LATEST: "claude-3-5-haiku-latest",
  /** Claude 3.5 Sonnet (latest) — Anthropic. */
  CLAUDE_3_5_SONNET_LATEST: "claude-3-5-sonnet-latest",
  /** Claude Sonnet 4.6 — Anthropic (1M context, reasoning-capable). */
  CLAUDE_SONNET_4_6: "claude-sonnet-4-6",
  /** DeepSeek V4 Flash — DeepSeek (non-thinking, fast). */
  DEEPSEEK_V4_FLASH: "deepseek-v4-flash",
  /** DeepSeek V4 Pro — DeepSeek (reasoning-heavy). */
  DEEPSEEK_V4_PRO: "deepseek-v4-pro",
  /** GPT-4.1 — OpenAI. */
  GPT_4_1: "gpt-4.1",
  /** GPT-4o mini — OpenAI, or via OpenRouter (`provider: Providers.OPENROUTER`). */
  GPT_4O_MINI: "gpt-4o-mini",
  /** GPT-4o — via OpenRouter (`provider: Providers.OPENROUTER`). */
  GPT_4O: "gpt-4o",
  /** Gemini 2.0 Flash — Gemini, or via OpenRouter (`provider: Providers.OPENROUTER`). */
  GEMINI_2_0_FLASH: "gemini-2.0-flash",
  /** Gemini 2.5 Flash — Gemini. */
  GEMINI_2_5_FLASH: "gemini-2.5-flash",
  /** Mistral Large (latest) — Mistral. */
  MISTRAL_LARGE_LATEST: "mistral-large-latest",
  /** Mistral Small (latest) — Mistral. */
  MISTRAL_SMALL_LATEST: "mistral-small-latest",
  /**
   * Doubao Seed 1.8 — ByteDance, via the official Ark API. Default routes
   * through the international BytePlus gateway (`Providers.DOUBAO`); pair with
   * `Providers.DOUBAO_CN` for the China Volcengine gateway.
   */
  DOUBAO_SEED_PRO: "doubao-seed-pro",
  /**
   * Doubao Seed 1.6 Flash — ByteDance, via the official Ark API (fast/cheap).
   * Default `Providers.DOUBAO` (international); pair with `Providers.DOUBAO_CN`
   * for China.
   */
  DOUBAO_SEED_FLASH: "doubao-seed-flash"
} as const satisfies Readonly<Record<string, RunModel>>;

/**
 * Per-model provider lists, in declaration order. Derived from
 * {@link MODEL_PROVIDER_IDS} so the two never drift.
 */
const PROVIDERS_BY_MODEL: Readonly<Record<RunModel, readonly RunProvider[]>> = (() => {
  const map = {} as Record<RunModel, readonly RunProvider[]>;
  for (const [model, providers] of Object.entries(MODEL_PROVIDER_IDS) as readonly [
    RunModel,
    Partial<Record<RunProvider, string>>
  ][]) {
    map[model] = Object.keys(providers) as RunProvider[];
  }
  return map;
})();

/**
 * Provider → canonical models that provider can serve. Derived from
 * {@link MODEL_PROVIDER_IDS}; every provider currently serves at least one
 * model, so all {@link RunProvider} keys are present.
 */
export const RUN_MODELS_BY_PROVIDER: Readonly<Record<RunProvider, readonly RunModel[]>> = (() => {
  const map = {} as Record<RunProvider, RunModel[]>;
  for (const [model, providers] of Object.entries(MODEL_PROVIDER_IDS) as readonly [
    RunModel,
    Partial<Record<RunProvider, string>>
  ][]) {
    for (const provider of Object.keys(providers) as RunProvider[]) {
      (map[provider] ??= []).push(model);
    }
  }
  return map;
})();

/**
 * All upstream providers that can serve a model id, in declaration order.
 * Returns `[]` for an unknown model.
 */
export function providersForModel(model: string): readonly RunProvider[] {
  return PROVIDERS_BY_MODEL[model as RunModel] ?? [];
}

/**
 * The default upstream provider for a model id — the first provider declared
 * for it in {@link MODEL_PROVIDER_IDS}. Returns `undefined` when the input is
 * not a known {@link RunModel} (so the SDK can fall back to the default and let
 * the server reject the model).
 */
export function providerForModel(model: string): RunProvider | undefined {
  return providersForModel(model)[0];
}

/**
 * Translate a canonical model id + provider into the provider-native model
 * string the upstream API expects (e.g. `("gpt-4o-mini", "openrouter")` →
 * `"openai/gpt-4o-mini"`). Throws when the provider does not serve the model.
 */
export function resolveProviderModelId(model: string, provider: RunProvider): string {
  const entry = MODEL_PROVIDER_IDS[model as RunModel] as Partial<Record<RunProvider, string>> | undefined;
  const native = entry?.[provider];
  if (native === undefined) {
    throw new Error(
      `resolveProviderModelId: model ${JSON.stringify(model)} is not available for provider ${JSON.stringify(provider)}; ` +
        `available: ${providersForModel(model).join(", ") || "(none)"}`
    );
  }
  return native;
}

/**
 * The single model→provider resolver shared by the SDK and the CLI, wrapping
 * {@link providersForModel} with the forward-compat unknown-model allowance:
 *
 *   - `provider` given: it is honored. If `model` is a KNOWN id, the provider
 *     must serve it (else throw). If `model` is UNKNOWN, it is allowed through
 *     so a slightly-old client can still run a newly-launched model (the server
 *     arbitrates).
 *   - `provider` omitted: a known model resolves to its DEFAULT provider (first
 *     declared). An UNKNOWN model with no provider throws a `did you mean?`
 *     hint — you must name a provider to run a model this client doesn't know.
 *
 * Returns the resolved {@link RunProvider}.
 */
export function resolveModelProvider(model: string, provider?: RunProvider): RunProvider {
  const providers = providersForModel(model);
  if (provider !== undefined) {
    if (providers.length > 0 && !providers.includes(provider)) {
      throw new Error(
        `model ${JSON.stringify(model)} is not available for provider ${provider}; ` +
          `available: ${providers.join(", ")}`
      );
    }
    return provider;
  }
  const inferred = providers[0];
  if (inferred === undefined) {
    const hint = suggest(model, RUN_MODELS);
    throw new Error(
      `${JSON.stringify(model)} is not a known model id` +
        (hint ? ` (did you mean ${JSON.stringify(hint)}?)` : "") +
        "; pass provider explicitly to run it"
    );
  }
  return inferred;
}

export function isRunModel(input: unknown): input is RunModel {
  return typeof input === "string" && (RUN_MODELS as readonly string[]).includes(input);
}

export function parseRunModel(input: unknown, field = "submission.model"): RunModel {
  if (!isRunModel(input)) {
    throw new Error(`${field} must be one of: ${RUN_MODELS.join(", ")}`);
  }
  return input;
}

export function assertRunModelMatchesProvider(
  provider: RunProvider,
  model: RunModel,
  field = "submission.model"
): void {
  const providers = providersForModel(model);
  if (!providers.includes(provider)) {
    throw new Error(
      `${field} ${JSON.stringify(model)} is not supported for provider ${provider}; ` +
        `expected one of: ${providers.join(", ")}`
    );
  }
}
