import type { RunProvider } from "./submission.js";

/**
 * Closed set of model ids accepted by the public run-submission schema.
 *
 * The provider-proxy still sends the model id through to the selected upstream
 * provider, but callers cannot submit arbitrary strings. Additions belong here
 * first so SDK types, CLI validation, docs examples, and platform parsing move
 * together.
 */
export const RUN_MODELS = [
  "claude-haiku-4-5",
  "claude-3-5-haiku-latest",
  "claude-3-5-sonnet-latest",
  "deepseek-v4-flash",
  "deepseek-v4-pro",
  "deepseek-chat",
  "deepseek-reasoner",
  "gpt-4.1",
  "gpt-4o-mini",
  "gemini-2.0-flash",
  "gemini-2.5-flash",
  "mistral-large-latest",
  "mistral-small-latest",
  "openai/gpt-4o-mini",
  "google/gemini-2.0-flash-001"
] as const;

export type RunModel = (typeof RUN_MODELS)[number];

/**
 * Symbol-style accessors for the closed model set. Prefer these over raw
 * strings so an invalid token is a compile error, not a runtime 400 — e.g.
 * `Models.CLAUDE_HAIKU_4_5`. The upstream provider is a pure function of the
 * model ({@link providerForModel}), so picking a model fully determines
 * routing; callers never pass `provider` separately.
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
  /** GPT-4o mini — OpenAI. */
  GPT_4O_MINI: "gpt-4o-mini",
  /** Gemini 2.0 Flash — Gemini. */
  GEMINI_2_0_FLASH: "gemini-2.0-flash",
  /** Gemini 2.5 Flash — Gemini. */
  GEMINI_2_5_FLASH: "gemini-2.5-flash",
  /** Mistral Large (latest) — Mistral. */
  MISTRAL_LARGE_LATEST: "mistral-large-latest",
  /** Mistral Small (latest) — Mistral. */
  MISTRAL_SMALL_LATEST: "mistral-small-latest",
  /** GPT-4o mini via OpenRouter (provider-prefixed) — cheap, tool-obedient. */
  OPENROUTER_GPT_4O_MINI: "openai/gpt-4o-mini",
  /** Gemini 2.0 Flash via OpenRouter (provider-prefixed) — cheap, tool-obedient. */
  OPENROUTER_GEMINI_2_0_FLASH: "google/gemini-2.0-flash-001"
} as const satisfies Readonly<Record<string, RunModel>>;

/**
 * Back-compat alias for {@link Models}. Existing imports of `RunModels`
 * keep working; new code should prefer `Models`.
 */
export const RunModels = Models;

export const RUN_MODELS_BY_PROVIDER = {
  anthropic: [
    Models.CLAUDE_HAIKU_4_5,
    Models.CLAUDE_3_5_HAIKU_LATEST,
    Models.CLAUDE_3_5_SONNET_LATEST
  ],
  deepseek: [
    Models.DEEPSEEK_V4_FLASH,
    Models.DEEPSEEK_V4_PRO,
    Models.DEEPSEEK_CHAT,
    Models.DEEPSEEK_REASONER
  ],
  openai: [Models.GPT_4_1, Models.GPT_4O_MINI],
  gemini: [Models.GEMINI_2_0_FLASH, Models.GEMINI_2_5_FLASH],
  mistral: [Models.MISTRAL_LARGE_LATEST, Models.MISTRAL_SMALL_LATEST],
  openrouter: [Models.OPENROUTER_GPT_4O_MINI, Models.OPENROUTER_GEMINI_2_0_FLASH]
} as const satisfies Readonly<Record<RunProvider, readonly RunModel[]>>;

/**
 * Reverse index: every model id → its single upstream provider. Derived from
 * {@link RUN_MODELS_BY_PROVIDER} so the two never drift; each model appears
 * under exactly one provider.
 */
const PROVIDER_BY_MODEL: Readonly<Record<RunModel, RunProvider>> = (() => {
  const map = {} as Record<RunModel, RunProvider>;
  for (const [provider, models] of Object.entries(RUN_MODELS_BY_PROVIDER) as readonly [
    RunProvider,
    readonly RunModel[]
  ][]) {
    for (const model of models) {
      map[model] = provider;
    }
  }
  return map;
})();

/**
 * Resolve the upstream provider for a model id. Returns `undefined` when the
 * input is not a known {@link RunModel} (so the SDK can fall back to the
 * default and let the server reject the model). Total over the closed model
 * set, so any `RunModel` resolves.
 */
export function providerForModel(model: string): RunProvider | undefined {
  return PROVIDER_BY_MODEL[model as RunModel];
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
  if (!(RUN_MODELS_BY_PROVIDER[provider] as readonly RunModel[]).includes(model)) {
    throw new Error(
      `${field} ${JSON.stringify(model)} is not supported for provider ${provider}; ` +
        `expected one of: ${RUN_MODELS_BY_PROVIDER[provider].join(", ")}`
    );
  }
}
