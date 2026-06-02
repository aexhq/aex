import type { RunProvider, RuntimeKind } from "./submission.js";

export const PROVIDER_SUPPORT_STATUSES = [
  "supported",
  "live-unverified",
  "provider-inherited",
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

const COMMON_DOCS = [
  { label: "Credentials", href: "credentials.md" },
  { label: "Events", href: "events.md" }
] as const satisfies readonly SupportPointer[];

const COMMON_EVIDENCE = [
  { label: "Submission parser and routing parity", href: "../../contracts/test/submission.test.ts" },
  { label: "Runtime support validator", href: "../../contracts/test/runtime-support.test.ts" },
  { label: "Generated matrix freshness", href: "../../../scripts/validate/capability-matrix.test.ts" }
] as const satisfies readonly SupportPointer[];

const LIVE_USER_MATRIX_EVIDENCE = [
  { label: "Installed-SDK live user matrix", href: "../../../apps/user-tests/test/live/live-sdk-comprehensive.test.ts" }
] as const satisfies readonly SupportPointer[];

const NATIVE_LIVE_EVIDENCE = [
  ...LIVE_USER_MATRIX_EVIDENCE,
  { label: "Native feature parity gate", href: "../../contracts/test/native-feature-gate.test.ts" }
] as const satisfies readonly SupportPointer[];

const MANAGED_PROXY_EVIDENCE = [
  ...LIVE_USER_MATRIX_EVIDENCE,
  { label: "Runtime support validator", href: "../../contracts/test/runtime-support.test.ts" }
] as const satisfies readonly SupportPointer[];

/**
 * Public provider support facts for generated SDK docs. Keep this metadata
 * public-facing only: provider names, support status, docs anchors, and
 * evidence pointers. Runtime routing and executor facts live separately in
 * `provider-capability.ts`.
 */
export const PROVIDER_PUBLIC_SUPPORT = {
  anthropic: {
    displayName: "Anthropic",
    status: "supported",
    docsAnchor: "anthropic",
    docs: COMMON_DOCS,
    evidence: [
      ...COMMON_EVIDENCE,
      {
        label: "Native feature parity gate",
        href: "../../contracts/test/native-feature-gate.test.ts"
      }
    ],
    runtimeEvidence: {
      native: NATIVE_LIVE_EVIDENCE,
      managed: MANAGED_PROXY_EVIDENCE
    }
  },
  deepseek: {
    displayName: "DeepSeek",
    status: "supported",
    docsAnchor: "deepseek",
    docs: COMMON_DOCS,
    evidence: [...COMMON_EVIDENCE, ...MANAGED_PROXY_EVIDENCE],
    runtimeEvidence: {
      managed: MANAGED_PROXY_EVIDENCE
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
  }
} as const satisfies Readonly<Record<RunProvider, ProviderPublicSupport>>;

export function providerPublicSupport(provider: RunProvider): ProviderPublicSupport {
  return PROVIDER_PUBLIC_SUPPORT[provider];
}
