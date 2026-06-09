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
  "deepseek-chat",
  "gpt-4.1",
  "gpt-4o-mini",
  "gemini-2.0-flash",
  "gemini-2.5-flash",
  "mistral-large-latest",
  "mistral-small-latest"
] as const;

export type RunModel = (typeof RUN_MODELS)[number];

export const RunModels = {
  CLAUDE_HAIKU_4_5: "claude-haiku-4-5",
  CLAUDE_3_5_HAIKU_LATEST: "claude-3-5-haiku-latest",
  CLAUDE_3_5_SONNET_LATEST: "claude-3-5-sonnet-latest",
  DEEPSEEK_CHAT: "deepseek-chat",
  GPT_4_1: "gpt-4.1",
  GPT_4O_MINI: "gpt-4o-mini",
  GEMINI_2_0_FLASH: "gemini-2.0-flash",
  GEMINI_2_5_FLASH: "gemini-2.5-flash",
  MISTRAL_LARGE_LATEST: "mistral-large-latest",
  MISTRAL_SMALL_LATEST: "mistral-small-latest"
} as const satisfies Readonly<Record<string, RunModel>>;

export const RUN_MODELS_BY_PROVIDER = {
  anthropic: [
    RunModels.CLAUDE_HAIKU_4_5,
    RunModels.CLAUDE_3_5_HAIKU_LATEST,
    RunModels.CLAUDE_3_5_SONNET_LATEST
  ],
  deepseek: [RunModels.DEEPSEEK_CHAT],
  openai: [RunModels.GPT_4_1, RunModels.GPT_4O_MINI],
  gemini: [RunModels.GEMINI_2_0_FLASH, RunModels.GEMINI_2_5_FLASH],
  mistral: [RunModels.MISTRAL_LARGE_LATEST, RunModels.MISTRAL_SMALL_LATEST]
} as const satisfies Readonly<Record<RunProvider, readonly RunModel[]>>;

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
