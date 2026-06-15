import type { RunProvider } from "./submission.js";

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
  "deepseek-v4-flash": { deepseek: "deepseek-v4-flash" },
  "deepseek-v4-pro": { deepseek: "deepseek-v4-pro" },
  "deepseek-chat": { deepseek: "deepseek-chat" },
  "deepseek-reasoner": { deepseek: "deepseek-reasoner" },
  "gpt-4.1": { openai: "gpt-4.1" },
  "gpt-4o-mini": { openai: "gpt-4o-mini", openrouter: "openai/gpt-4o-mini" },
  "gpt-4o": { openrouter: "openai/gpt-4o" },
  "gemini-2.0-flash": { gemini: "gemini-2.0-flash", openrouter: "google/gemini-2.0-flash-001" },
  "gemini-2.5-flash": { gemini: "gemini-2.5-flash" },
  "mistral-large-latest": { mistral: "mistral-large-latest" },
  "mistral-small-latest": { mistral: "mistral-small-latest" }
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
  /** DeepSeek V4 Flash — DeepSeek (non-thinking, fast). */
  DEEPSEEK_V4_FLASH: "deepseek-v4-flash",
  /** DeepSeek V4 Pro — DeepSeek (reasoning-heavy). */
  DEEPSEEK_V4_PRO: "deepseek-v4-pro",
  /**
   * @deprecated Legacy alias DeepSeek routes to `deepseek-v4-flash` (non-thinking).
   * DeepSeek removes this name on 2026-07-24 15:59 UTC — migrate to
   * {@link Models.DEEPSEEK_V4_FLASH}.
   */
  DEEPSEEK_CHAT: "deepseek-chat",
  /**
   * @deprecated Legacy alias DeepSeek routes to `deepseek-v4-flash` (thinking).
   * DeepSeek removes this name on 2026-07-24 15:59 UTC — migrate to
   * {@link Models.DEEPSEEK_V4_FLASH} (or {@link Models.DEEPSEEK_V4_PRO} for
   * heavier reasoning).
   */
  DEEPSEEK_REASONER: "deepseek-reasoner",
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
  MISTRAL_SMALL_LATEST: "mistral-small-latest"
} as const satisfies Readonly<Record<string, RunModel>>;

/**
 * Back-compat alias for {@link Models}. Existing imports of `RunModels`
 * keep working; new code should prefer `Models`.
 */
export const RunModels = Models;

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
