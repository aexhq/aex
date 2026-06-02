import type { RunProvider } from "./submission.js";

/**
 * Providers whose runtime is implemented by a native (provider-hosted)
 * agent runtime. Every other provider falls back to Goose Managed.
 *
 * This routing list mirrors {@link PROVIDER_CAPABILITY}; shared tests
 * assert that the two do not drift.
 */
export const NATIVE_RUNTIME_PROVIDERS = ["anthropic"] as const satisfies readonly RunProvider[];
export type NativeRuntimeProvider = (typeof NATIVE_RUNTIME_PROVIDERS)[number];

/**
 * Which native executor implements a provider's hosted agent runtime.
 * One id per native runtime; the dispatcher resolves it from the run's
 * provider. Goose Managed is the universal fallback and has no id here.
 */
export type NativeExecutorId = "anthropic-managed";

/**
 * What a provider's native agent runtime can do. `serves` declares which
 * submission features the native path can honour today; the feature-level
 * fail-closed gate reads it, so flipping a flag changes native admission.
 * Provider built-in skills (`Skill.provider(...)`) are native-only and are
 * gated in the managed path instead.
 */
export interface NativeAgentCapability {
  readonly executor: NativeExecutorId;
  readonly serves: {
    readonly inlineSkills: boolean;
    readonly files: boolean;
    readonly mcpServers: boolean;
  };
}

export interface ProviderCapability {
  /** `null` means no native agent runtime, so this provider uses Goose Managed. */
  readonly nativeAgent: NativeAgentCapability | null;
}

/**
 * Per-provider runtime routing capability. This is intentionally limited to
 * routing/admission facts. Public support labels, docs anchors, and evidence
 * links live in `provider-support.ts` so generated docs do not mix product
 * support statements with dispatcher implementation facts.
 */
export const PROVIDER_CAPABILITY = {
  anthropic: {
    nativeAgent: {
      executor: "anthropic-managed",
      serves: { inlineSkills: true, files: true, mcpServers: true }
    }
  },
  deepseek: { nativeAgent: null },
  openai: { nativeAgent: null },
  gemini: { nativeAgent: null },
  mistral: { nativeAgent: null }
} as const satisfies Readonly<Record<RunProvider, ProviderCapability>>;

/** True when the provider has a native agent runtime, not the Goose fallback. */
export function providerHasNativeAgent(provider: RunProvider): boolean {
  return PROVIDER_CAPABILITY[provider].nativeAgent !== null;
}
