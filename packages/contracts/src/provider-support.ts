import type { RunProvider } from "./submission.js";

export interface SupportPointer {
  readonly label: string;
  /** Markdown href, relative to `packages/sdk/docs/provider-runtime-capabilities.md`. */
  readonly href: string;
}

export interface ProviderPublicSupport {
  readonly displayName: string;
  readonly docsAnchor: string;
  readonly docs: readonly SupportPointer[];
  readonly evidence: readonly SupportPointer[];
  readonly managedEvidence: readonly SupportPointer[];
}

const COMMON_DOCS = [
  { label: "Secrets", href: "secrets.md" },
  { label: "Events", href: "events.md" }
] as const satisfies readonly SupportPointer[];

const COMMON_EVIDENCE = [
  { label: "Submission parser and routing parity", href: "../../contracts/test/submission.test.ts" },
  { label: "Generated matrix freshness", href: "../../../scripts/validate/capability-matrix.test.ts" }
] as const satisfies readonly SupportPointer[];

const ANTHROPIC_LIVE_USER_EVIDENCE = [
  {
    label: "Installed-SDK Anthropic live user test",
    href: "../../../apps/user-tests/test/live/providers/live-sdk-anthropic-managed.test.ts"
  }
] as const satisfies readonly SupportPointer[];

const DEEPSEEK_LIVE_USER_EVIDENCE = [
  {
    label: "Installed-SDK DeepSeek live user test",
    href: "../../../apps/user-tests/test/live/live-sdk-deepseek.test.ts"
  },
  {
    label: "Installed-SDK DeepSeek comprehensive live user matrix",
    href: "../../../apps/user-tests/test/live/live-sdk-comprehensive.test.ts"
  }
] as const satisfies readonly SupportPointer[];

const ANTHROPIC_MANAGED_EVIDENCE = ANTHROPIC_LIVE_USER_EVIDENCE;

const DEEPSEEK_MANAGED_EVIDENCE = DEEPSEEK_LIVE_USER_EVIDENCE;

/**
 * Public provider support facts for generated SDK docs. Keep this metadata
 * public-facing only: provider names, docs anchors, and evidence pointers.
 * Inclusion in this registry means the provider is supported.
 */
export const PROVIDER_PUBLIC_SUPPORT = {
  anthropic: {
    displayName: "Anthropic",
    docsAnchor: "anthropic",
    docs: COMMON_DOCS,
    evidence: [...COMMON_EVIDENCE, ...ANTHROPIC_MANAGED_EVIDENCE],
    managedEvidence: ANTHROPIC_MANAGED_EVIDENCE
  },
  deepseek: {
    displayName: "DeepSeek",
    docsAnchor: "deepseek",
    docs: COMMON_DOCS,
    evidence: [...COMMON_EVIDENCE, ...DEEPSEEK_MANAGED_EVIDENCE],
    managedEvidence: DEEPSEEK_MANAGED_EVIDENCE
  },
  openai: {
    displayName: "OpenAI",
    docsAnchor: "openai",
    docs: COMMON_DOCS,
    evidence: COMMON_EVIDENCE,
    managedEvidence: COMMON_EVIDENCE
  },
  gemini: {
    displayName: "Gemini",
    docsAnchor: "gemini",
    docs: COMMON_DOCS,
    evidence: COMMON_EVIDENCE,
    managedEvidence: COMMON_EVIDENCE
  },
  mistral: {
    displayName: "Mistral",
    docsAnchor: "mistral",
    docs: COMMON_DOCS,
    evidence: COMMON_EVIDENCE,
    managedEvidence: COMMON_EVIDENCE
  },
  openrouter: {
    displayName: "OpenRouter",
    docsAnchor: "openrouter",
    docs: COMMON_DOCS,
    evidence: COMMON_EVIDENCE,
    managedEvidence: COMMON_EVIDENCE
  },
  // Doubao (ByteDance) via the official Ark API — international BytePlus gateway.
  doubao: {
    displayName: "Doubao",
    docsAnchor: "doubao",
    docs: COMMON_DOCS,
    evidence: COMMON_EVIDENCE,
    managedEvidence: COMMON_EVIDENCE
  },
  // Doubao (ByteDance) via the official Ark API — China Volcengine gateway.
  "doubao-cn": {
    displayName: "Doubao (China)",
    docsAnchor: "doubao-cn",
    docs: COMMON_DOCS,
    evidence: COMMON_EVIDENCE,
    managedEvidence: COMMON_EVIDENCE
  }
} as const satisfies Readonly<Record<RunProvider, ProviderPublicSupport>>;

export function providerPublicSupport(provider: RunProvider): ProviderPublicSupport {
  return PROVIDER_PUBLIC_SUPPORT[provider];
}
