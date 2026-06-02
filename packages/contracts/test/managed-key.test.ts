import { describe, expect, it } from "vitest";
import {
  BLOCKED_MANAGED_KEY_FEATURE_POLICY_V1,
  BLOCKED_MANAGED_KEY_POLICY_V1,
  DEFAULT_CREDENTIAL_MODE,
  FakeManagedCredentialResolver,
  ManagedKeyUnavailableError,
  assertManagedKeyAdmissionAllowed,
  assertManagedKeyModeAvailable,
  credentialModeOrDefault,
  isManagedKeyAdmissionAllowed,
  isManagedKeyGenerallyAvailable,
  parseCredentialMode,
  type ManagedKeyPolicyV1
} from "../src/index.js";

const availablePolicy = {
  schemaVersion: 1,
  credentialMode: "managed",
  launchStage: "ga",
  serviceAvailable: true,
  billingRequired: true,
  providers: ["anthropic"],
  runtimes: ["managed"],
  models: ["claude-haiku-4-5"],
  features: {
    ...BLOCKED_MANAGED_KEY_FEATURE_POLICY_V1,
    files: "allowed"
  }
} satisfies ManagedKeyPolicyV1;

describe("managed-key public contract", () => {
  it("defaults credentialMode to BYOK", () => {
    expect(DEFAULT_CREDENTIAL_MODE).toBe("byok");
    expect(parseCredentialMode(undefined)).toBe("byok");
    expect(credentialModeOrDefault(undefined)).toBe("byok");
    expect(parseCredentialMode("managed")).toBe("managed");
    expect(() => parseCredentialMode("workspace-secret")).toThrow(/credentialMode must be one of/);
  });

  it("keeps the public managed-key policy blocked by default", () => {
    expect(BLOCKED_MANAGED_KEY_POLICY_V1).toMatchObject({
      credentialMode: "managed",
      launchStage: "blocked",
      serviceAvailable: false,
      billingRequired: true,
      providers: [],
      runtimes: []
    });
    expect(isManagedKeyGenerallyAvailable(BLOCKED_MANAGED_KEY_POLICY_V1)).toBe(false);
    expect(() => assertManagedKeyModeAvailable(BLOCKED_MANAGED_KEY_POLICY_V1)).toThrow(
      ManagedKeyUnavailableError
    );
  });

  it("does not mark GA available without the service implementation flag", () => {
    const policy = {
      ...availablePolicy,
      serviceAvailable: false
    } satisfies ManagedKeyPolicyV1;

    expect(policy.launchStage).toBe("ga");
    expect(isManagedKeyGenerallyAvailable(policy)).toBe(false);
  });

  it("separates injected admission allowance from public GA availability", () => {
    const policy = {
      ...availablePolicy,
      launchStage: "pilot"
    } satisfies ManagedKeyPolicyV1;

    expect(isManagedKeyGenerallyAvailable(policy)).toBe(false);
    expect(isManagedKeyAdmissionAllowed(policy)).toBe(true);
    expect(() => assertManagedKeyModeAvailable(policy)).toThrow(ManagedKeyUnavailableError);
    expect(() => assertManagedKeyAdmissionAllowed(policy)).not.toThrow();
  });

  it("fake resolver denies the blocked public policy", async () => {
    const resolver = new FakeManagedCredentialResolver();
    await expect(
      resolver.resolveManagedCredential({
        workspaceId: "workspace-1",
        runId: "run-1",
        provider: "anthropic",
        runtime: "managed",
        model: "claude-haiku-4-5",
        policy: BLOCKED_MANAGED_KEY_POLICY_V1
      })
    ).resolves.toMatchObject({ ok: false, code: "managed_key_unavailable" });
  });

  it("fake resolver returns only a public lease shape when policy allows it", async () => {
    const resolver = new FakeManagedCredentialResolver();
    const result = await resolver.resolveManagedCredential({
      workspaceId: "workspace-1",
      runId: "run-1",
      provider: "anthropic",
      runtime: "managed",
      model: "claude-haiku-4-5",
      policy: availablePolicy
    });

    expect(result).toMatchObject({ ok: true });
    if (!result.ok) throw new Error("expected fake resolver success");
    expect(Object.keys(result.lease).sort()).toEqual([
      "credentialMode",
      "custodyClass",
      "provider",
      "runtime"
    ]);
    expect(JSON.stringify(result.lease)).not.toMatch(/sk-|bearer|account/i);
  });

  it("fake resolver enforces public provider, runtime, and model allowlists", async () => {
    const resolver = new FakeManagedCredentialResolver();
    const base = {
      workspaceId: "workspace-1",
      runId: "run-1",
      provider: "anthropic" as const,
      runtime: "managed" as const,
      model: "claude-haiku-4-5",
      policy: availablePolicy
    };

    await expect(
      resolver.resolveManagedCredential({ ...base, provider: "deepseek" })
    ).resolves.toMatchObject({ ok: false, code: "provider_not_allowed" });
    await expect(
      resolver.resolveManagedCredential({ ...base, policy: { ...availablePolicy, runtimes: [] } })
    ).resolves.toMatchObject({ ok: false, code: "runtime_not_allowed" });
    await expect(
      resolver.resolveManagedCredential({ ...base, model: "other-model" })
    ).resolves.toMatchObject({ ok: false, code: "model_not_allowed" });
  });
});
