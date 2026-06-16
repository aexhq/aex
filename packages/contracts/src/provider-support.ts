import type { RunProvider, RuntimeKind, RuntimeValidationCode } from "./submission.js";

export const PROVIDER_SUPPORT_STATUSES = [
  "supported",
  "live-unverified",
  "rejected"
] as const;
export type ProviderSupportStatus = (typeof PROVIDER_SUPPORT_STATUSES)[number];

export interface SupportPointer {
  readonly label: string;
  /** Markdown href, relative to `packages/sdk/docs/provider-runtime-capabilities.md`. */
  readonly href: string;
}

export interface ProviderPublicSupport {
  readonly displayName: string;
  readonly status: ProviderSupportStatus;
  readonly docsAnchor: string;
  readonly docs: readonly SupportPointer[];
  readonly evidence: readonly SupportPointer[];
  readonly runtimeEvidence: Readonly<Partial<Record<RuntimeKind, readonly SupportPointer[]>>>;
}

export interface RuntimeValidationSupport {
  readonly docsAnchor: string;
  readonly docs: readonly SupportPointer[];
  readonly evidence: readonly SupportPointer[];
  readonly enforcement: string;
}

const COMMON_DOCS = [
  { label: "Credentials", href: "credentials.md" },
  { label: "Events", href: "events.md" }
] as const satisfies readonly SupportPointer[];

const COMMON_EVIDENCE = [
  { label: "Submission parser and routing parity", href: "../../contracts/test/submission.test.ts" },
  { label: "Runtime support validator", href: "../../contracts/test/runtime-support.test.ts" },
  { label: "Generated matrix freshness", href: "../../../scripts/validate/capability-matrix.test.ts" }
] as const satisfies readonly SupportPointer[];

const ANTHROPIC_LIVE_USER_EVIDENCE = [
  {
    label: "Installed-SDK Anthropic live user test",
    href: "../../../apps/user-tests/test/live/live-sdk-anthropic-managed.test.ts"
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

const ANTHROPIC_MANAGED_EVIDENCE = [
  ...ANTHROPIC_LIVE_USER_EVIDENCE,
  { label: "Runtime support validator", href: "../../contracts/test/runtime-support.test.ts" }
] as const satisfies readonly SupportPointer[];

const DEEPSEEK_MANAGED_EVIDENCE = [
  ...DEEPSEEK_LIVE_USER_EVIDENCE,
  { label: "Runtime support validator", href: "../../contracts/test/runtime-support.test.ts" }
] as const satisfies readonly SupportPointer[];

export const RUNTIME_VALIDATION_SUPPORT = {
  feature_runtime_mismatch: {
    docsAnchor: "managed-unsupported-features",
    docs: [{ label: "Runtime routing", href: "provider-runtime-capabilities.md#runtime-routing" }],
    evidence: [{ label: "Submission parser and routing parity", href: "../../contracts/test/submission.test.ts" }],
    enforcement: "collectManagedUnsupportedFeatures + selectRuntime"
  }
} as const satisfies Readonly<Record<RuntimeValidationCode, RuntimeValidationSupport>>;

/**
 * Public provider support facts for generated SDK docs. Keep this metadata
 * public-facing only: provider names, support status, docs anchors, and
 * evidence pointers.
 */
export const PROVIDER_PUBLIC_SUPPORT = {
  anthropic: {
    displayName: "Anthropic",
    status: "supported",
    docsAnchor: "anthropic",
    docs: COMMON_DOCS,
    evidence: [...COMMON_EVIDENCE, ...ANTHROPIC_MANAGED_EVIDENCE],
    runtimeEvidence: {
      managed: ANTHROPIC_MANAGED_EVIDENCE
    }
  },
  deepseek: {
    displayName: "DeepSeek",
    status: "supported",
    docsAnchor: "deepseek",
    docs: COMMON_DOCS,
    evidence: [...COMMON_EVIDENCE, ...DEEPSEEK_MANAGED_EVIDENCE],
    runtimeEvidence: {
      managed: DEEPSEEK_MANAGED_EVIDENCE
    }
  },
  openai: {
    displayName: "OpenAI",
    status: "live-unverified",
    docsAnchor: "openai",
    docs: COMMON_DOCS,
    evidence: COMMON_EVIDENCE,
    runtimeEvidence: {
      managed: COMMON_EVIDENCE
    }
  },
  gemini: {
    displayName: "Gemini",
    status: "live-unverified",
    docsAnchor: "gemini",
    docs: COMMON_DOCS,
    evidence: COMMON_EVIDENCE,
    runtimeEvidence: {
      managed: COMMON_EVIDENCE
    }
  },
  mistral: {
    displayName: "Mistral",
    status: "live-unverified",
    docsAnchor: "mistral",
    docs: COMMON_DOCS,
    evidence: COMMON_EVIDENCE,
    runtimeEvidence: {
      managed: COMMON_EVIDENCE
    }
  },
  openrouter: {
    displayName: "OpenRouter",
    status: "live-unverified",
    docsAnchor: "openrouter",
    docs: COMMON_DOCS,
    evidence: COMMON_EVIDENCE,
    runtimeEvidence: {
      managed: COMMON_EVIDENCE
    }
  },
  // Doubao (ByteDance) via the official Ark API — international BytePlus gateway.
  // Wired + parser/routing-verified but not yet proven against a live BytePlus
  // account, so `live-unverified`. Promote to `supported` (and swap COMMON_EVIDENCE
  // for a DOUBAO_MANAGED_EVIDENCE pointer to live-sdk-doubao.test.ts) once the
  // live run passes.
  doubao: {
    displayName: "Doubao",
    status: "live-unverified",
    docsAnchor: "doubao",
    docs: COMMON_DOCS,
    evidence: COMMON_EVIDENCE,
    runtimeEvidence: {
      managed: COMMON_EVIDENCE
    }
  },
  // Doubao (ByteDance) via the official Ark API — China Volcengine gateway.
  // Same wiring as `doubao`; additionally gated on CF Worker egress reaching the
  // Beijing host (see apps/egress-probe). `live-unverified` until proven live.
  "doubao-cn": {
    displayName: "Doubao (China)",
    status: "live-unverified",
    docsAnchor: "doubao-cn",
    docs: COMMON_DOCS,
    evidence: COMMON_EVIDENCE,
    runtimeEvidence: {
      managed: COMMON_EVIDENCE
    }
  }
} as const satisfies Readonly<Record<RunProvider, ProviderPublicSupport>>;

export function providerPublicSupport(provider: RunProvider): ProviderPublicSupport {
  return PROVIDER_PUBLIC_SUPPORT[provider];
}
