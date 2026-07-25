import { withContractParseError } from "./contract-parse-error.js";
import {
  RUNTIME_SECURITY_PROFILES,
  RuntimeSecurityProfileSchema
} from "./schemas/runtime-security-profile.js";
import { parseWire } from "./schemas/wire.js";

export { RUNTIME_SECURITY_PROFILES };
export type RuntimeSecurityProfileName = (typeof RUNTIME_SECURITY_PROFILES)[number];

export interface RuntimeSecurityProfile {
  readonly name: RuntimeSecurityProfileName;
  readonly defaultNetworkingMode: "limited" | "open";
  readonly allowOpenNetworking: boolean;
  readonly allowRuntimePackages: boolean;
  readonly allowCustomerEnvVars: boolean;
  readonly allowMcpServers: boolean;
}

export interface RuntimeSecurityProfileEvaluationInput {
  readonly networkingMode?: "limited" | "open";
  readonly packageCount?: number;
  readonly customerEnvVarCount?: number;
  readonly mcpServerCount?: number;
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
      allowMcpServers: true
    }),
    standard: Object.freeze({
      name: "standard",
      defaultNetworkingMode: "open",
      allowOpenNetworking: true,
      allowRuntimePackages: true,
      allowCustomerEnvVars: true,
      allowMcpServers: true
    }),
    developer: Object.freeze({
      name: "developer",
      defaultNetworkingMode: "open",
      allowOpenNetworking: true,
      allowRuntimePackages: true,
      allowCustomerEnvVars: true,
      allowMcpServers: true
    })
  });

export function parseRuntimeSecurityProfile(input: unknown): RuntimeSecurityProfileName | undefined {
  return withContractParseError("parseRuntimeSecurityProfile", () => {
    if (input === undefined || input === null) return undefined;
    return parseWire(RuntimeSecurityProfileSchema, input);
  });
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
  if ((input.mcpServerCount ?? 0) > 0 && !profile.allowMcpServers) {
    violations.push({
      field: "submission.mcpServers",
      reason: `${profile.name} does not allow MCP servers`
    });
  }
  return Object.freeze(violations);
}
