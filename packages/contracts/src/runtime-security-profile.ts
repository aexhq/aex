export const RUNTIME_SECURITY_PROFILES = ["strict", "standard", "developer"] as const;
export type RuntimeSecurityProfileName = (typeof RUNTIME_SECURITY_PROFILES)[number];

export interface RuntimeSecurityProfile {
  readonly name: RuntimeSecurityProfileName;
  readonly defaultNetworkingMode: "limited" | "open";
  readonly allowOpenNetworking: boolean;
  readonly allowRuntimePackages: boolean;
  readonly allowCustomerEnvVars: boolean;
  readonly allowProxyEndpoints: boolean;
  readonly allowMcpServers: boolean;
  readonly allowRetainedSessions: boolean;
}

export interface RuntimeSecurityProfileEvaluationInput {
  readonly networkingMode?: "limited" | "open";
  readonly packageCount?: number;
  readonly customerEnvVarCount?: number;
  readonly proxyEndpointCount?: number;
  readonly mcpServerCount?: number;
  readonly cleanupSession?: "retain" | "delete";
}

export interface RuntimeSecurityProfileViolation {
  readonly field: string;
  readonly reason: string;
}

export const RUNTIME_SECURITY_PROFILE_CONFIG: Readonly<Record<RuntimeSecurityProfileName, RuntimeSecurityProfile>> =
  Object.freeze({
    strict: Object.freeze({
      name: "strict",
      defaultNetworkingMode: "limited",
      allowOpenNetworking: false,
      allowRuntimePackages: false,
      allowCustomerEnvVars: true,
      allowProxyEndpoints: true,
      allowMcpServers: true,
      allowRetainedSessions: false
    }),
    standard: Object.freeze({
      name: "standard",
      defaultNetworkingMode: "limited",
      allowOpenNetworking: true,
      allowRuntimePackages: true,
      allowCustomerEnvVars: true,
      allowProxyEndpoints: true,
      allowMcpServers: true,
      allowRetainedSessions: false
    }),
    developer: Object.freeze({
      name: "developer",
      defaultNetworkingMode: "open",
      allowOpenNetworking: true,
      allowRuntimePackages: true,
      allowCustomerEnvVars: true,
      allowProxyEndpoints: true,
      allowMcpServers: true,
      allowRetainedSessions: true
    })
  });

export function parseRuntimeSecurityProfile(input: unknown): RuntimeSecurityProfileName | undefined {
  if (input === undefined || input === null) {
    return undefined;
  }
  if (typeof input !== "string" || !(RUNTIME_SECURITY_PROFILES as readonly string[]).includes(input)) {
    throw new Error(
      `securityProfile must be one of: ${RUNTIME_SECURITY_PROFILES.join(", ")} (got ${JSON.stringify(input)})`
    );
  }
  return input as RuntimeSecurityProfileName;
}

export function resolveRuntimeSecurityProfile(
  input: RuntimeSecurityProfileName | undefined
): RuntimeSecurityProfile {
  return RUNTIME_SECURITY_PROFILE_CONFIG[input ?? "standard"];
}

export function serializeRuntimeSecurityProfile(profile: RuntimeSecurityProfileName): string {
  parseRuntimeSecurityProfile(profile);
  return profile;
}

export function evaluateRuntimeSecurityProfile(
  profileName: RuntimeSecurityProfileName | undefined,
  input: RuntimeSecurityProfileEvaluationInput
): readonly RuntimeSecurityProfileViolation[] {
  const profile = resolveRuntimeSecurityProfile(profileName);
  const violations: RuntimeSecurityProfileViolation[] = [];
  if (input.networkingMode === "open" && !profile.allowOpenNetworking) {
    violations.push({
      field: "environment.networking.mode",
      reason: `${profile.name} requires limited networking`
    });
  }
  if ((input.packageCount ?? 0) > 0 && !profile.allowRuntimePackages) {
    violations.push({
      field: "environment.packages",
      reason: `${profile.name} does not allow runtime package installs`
    });
  }
  if ((input.customerEnvVarCount ?? 0) > 0 && !profile.allowCustomerEnvVars) {
    violations.push({
      field: "environment.envVars",
      reason: `${profile.name} does not allow customer runtime env vars`
    });
  }
  if ((input.proxyEndpointCount ?? 0) > 0 && !profile.allowProxyEndpoints) {
    violations.push({
      field: "proxyEndpoints",
      reason: `${profile.name} does not allow HTTP proxy endpoints`
    });
  }
  if ((input.mcpServerCount ?? 0) > 0 && !profile.allowMcpServers) {
    violations.push({
      field: "submission.mcpServers",
      reason: `${profile.name} does not allow MCP servers`
    });
  }
  if (input.cleanupSession === "retain" && !profile.allowRetainedSessions) {
    violations.push({
      field: "cleanup.session",
      reason: `${profile.name} requires provider sessions to be cleaned up`
    });
  }
  return Object.freeze(violations);
}
