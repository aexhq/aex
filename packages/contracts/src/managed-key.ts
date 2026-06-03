import type { RunProvider, RuntimeKind } from "./submission.js";

export const CREDENTIAL_MODES = ["byok", "managed"] as const;
export type CredentialMode = (typeof CREDENTIAL_MODES)[number];
export const DEFAULT_CREDENTIAL_MODE: CredentialMode = "byok";

export const MANAGED_KEY_POLICY_SCHEMA_VERSION = 1;

export const MANAGED_KEY_LAUNCH_STAGES = ["blocked", "pilot", "ga"] as const;
export type ManagedKeyLaunchStage = (typeof MANAGED_KEY_LAUNCH_STAGES)[number];

export const MANAGED_KEY_FEATURE_DECISIONS = ["disabled", "allowed"] as const;
export type ManagedKeyFeatureDecision = (typeof MANAGED_KEY_FEATURE_DECISIONS)[number];

export interface ManagedKeyFeaturePolicyV1 {
  readonly files: ManagedKeyFeatureDecision;
  readonly packages: ManagedKeyFeatureDecision;
  readonly builtins: ManagedKeyFeatureDecision;
  readonly mcpServers: ManagedKeyFeatureDecision;
  readonly proxyEndpoints: ManagedKeyFeatureDecision;
  readonly openNetworking: ManagedKeyFeatureDecision;
}

/**
 * Public managed-key policy contract. Concrete policy values, account
 * selection, financial calculation, and deployment wiring live outside this
 * public module.
 */
export interface ManagedKeyPolicyV1 {
  readonly schemaVersion: typeof MANAGED_KEY_POLICY_SCHEMA_VERSION;
  readonly credentialMode: "managed";
  readonly launchStage: ManagedKeyLaunchStage;
  readonly serviceAvailable: boolean;
  readonly billingRequired: true;
  readonly providers: readonly RunProvider[];
  readonly runtimes: readonly RuntimeKind[];
  readonly models?: readonly string[];
  readonly features: ManagedKeyFeaturePolicyV1;
}

export const BLOCKED_MANAGED_KEY_FEATURE_POLICY_V1: ManagedKeyFeaturePolicyV1 = Object.freeze({
  files: "disabled",
  packages: "disabled",
  builtins: "disabled",
  mcpServers: "disabled",
  proxyEndpoints: "disabled",
  openNetworking: "disabled"
});

export const BLOCKED_MANAGED_KEY_POLICY_V1: ManagedKeyPolicyV1 = Object.freeze({
  schemaVersion: MANAGED_KEY_POLICY_SCHEMA_VERSION,
  credentialMode: "managed",
  launchStage: "blocked",
  serviceAvailable: false,
  billingRequired: true,
  providers: Object.freeze([]),
  runtimes: Object.freeze([]),
  features: BLOCKED_MANAGED_KEY_FEATURE_POLICY_V1
});

export class ManagedKeyUnavailableError extends Error {
  readonly code = "managed_key_unavailable";

  constructor(message = "credentialMode: \"managed\" is not available") {
    super(message);
    this.name = "ManagedKeyUnavailableError";
  }
}

export function parseCredentialMode(input: unknown): CredentialMode {
  if (input === undefined) {
    return DEFAULT_CREDENTIAL_MODE;
  }
  if (!isCredentialMode(input)) {
    throw new Error(
      `credentialMode must be one of: ${CREDENTIAL_MODES.join(", ")} (got ${JSON.stringify(input)})`
    );
  }
  return input;
}

export function credentialModeOrDefault(input: CredentialMode | undefined): CredentialMode {
  return input ?? DEFAULT_CREDENTIAL_MODE;
}

export function isCredentialMode(input: unknown): input is CredentialMode {
  return typeof input === "string" && (CREDENTIAL_MODES as readonly string[]).includes(input);
}

export function isManagedKeyGenerallyAvailable(policy: ManagedKeyPolicyV1): boolean {
  return policy.launchStage === "ga" && policy.serviceAvailable;
}

export function isManagedKeyAdmissionAllowed(policy: ManagedKeyPolicyV1): boolean {
  return policy.launchStage !== "blocked" && policy.serviceAvailable;
}

export function assertManagedKeyModeAvailable(policy: ManagedKeyPolicyV1 = BLOCKED_MANAGED_KEY_POLICY_V1): void {
  if (!isManagedKeyGenerallyAvailable(policy)) {
    throw new ManagedKeyUnavailableError();
  }
}

export function assertManagedKeyAdmissionAllowed(
  policy: ManagedKeyPolicyV1 = BLOCKED_MANAGED_KEY_POLICY_V1
): void {
  if (!isManagedKeyAdmissionAllowed(policy)) {
    throw new ManagedKeyUnavailableError();
  }
}

export interface ManagedCredentialResolutionInput {
  readonly workspaceId: string;
  readonly runId: string;
  readonly provider: RunProvider;
  readonly runtime: RuntimeKind;
  readonly model: string;
  readonly policy: ManagedKeyPolicyV1;
}

export interface ManagedCredentialLease {
  readonly credentialMode: "managed";
  readonly provider: RunProvider;
  readonly runtime: RuntimeKind;
  readonly custodyClass: "managed-provider-credential";
}

export type ManagedCredentialResolution =
  | {
      readonly ok: true;
      readonly lease: ManagedCredentialLease;
    }
  | {
      readonly ok: false;
      readonly code:
        | "managed_key_unavailable"
        | "provider_not_allowed"
        | "runtime_not_allowed"
        | "model_not_allowed";
      readonly message: string;
    };

export interface ManagedCredentialResolver {
  resolveManagedCredential(input: ManagedCredentialResolutionInput): Promise<ManagedCredentialResolution>;
}

export class FakeManagedCredentialResolver implements ManagedCredentialResolver {
  async resolveManagedCredential(input: ManagedCredentialResolutionInput): Promise<ManagedCredentialResolution> {
    const denial = resolvePolicyDenial(input);
    if (denial) {
      return denial;
    }
    return {
      ok: true,
      lease: Object.freeze({
        credentialMode: "managed",
        provider: input.provider,
        runtime: input.runtime,
        custodyClass: "managed-provider-credential"
      })
    };
  }
}

function resolvePolicyDenial(
  input: ManagedCredentialResolutionInput
): Extract<ManagedCredentialResolution, { ok: false }> | null {
  if (!isManagedKeyGenerallyAvailable(input.policy)) {
    return {
      ok: false,
      code: "managed_key_unavailable",
      message: "managed-key mode is not generally available for this public policy"
    };
  }
  if (!input.policy.providers.includes(input.provider)) {
    return {
      ok: false,
      code: "provider_not_allowed",
      message: `provider ${input.provider} is not allowed by managed-key policy`
    };
  }
  if (!input.policy.runtimes.includes(input.runtime)) {
    return {
      ok: false,
      code: "runtime_not_allowed",
      message: `runtime ${input.runtime} is not allowed by managed-key policy`
    };
  }
  if (input.policy.models && !input.policy.models.includes(input.model)) {
    return {
      ok: false,
      code: "model_not_allowed",
      message: "model is not allowed by managed-key policy"
    };
  }
  return null;
}
