import type {
  McpServerRef,
  PlatformEnvironmentInput,
  PlatformMcpServerSecret,
  PlatformSubmission,
  SessionRetentionPolicy
} from "@aexhq/contracts";
import type { SessionCreateOptions, SessionEnvironmentOptions } from "./client-types.js";
import { McpServer } from "./mcp-server.js";
import { configError } from "./session-validate.js";

export function fileCaptureForWire(
  fileCapture: SessionCreateOptions["fileCapture"]
): PlatformSubmission["fileCapture"] | undefined {
  if (fileCapture === undefined) {
    return undefined;
  }
  const allowedDirs = fileCapture.allowedDirs?.filter((dir) => dir.length > 0);
  const deniedDirs = fileCapture.deniedDirs?.filter((dir) => dir.length > 0);
  const hasNumericOverride =
    fileCapture.captureTimeoutMs !== undefined ||
    fileCapture.maxFileBytes !== undefined ||
    fileCapture.maxTotalBytes !== undefined ||
    fileCapture.maxFiles !== undefined;
  if ((allowedDirs?.length ?? 0) === 0 && (deniedDirs?.length ?? 0) === 0 && !hasNumericOverride) {
    return undefined;
  }
  return {
    ...(allowedDirs && allowedDirs.length > 0 ? { allowedDirs } : {}),
    ...(deniedDirs && deniedDirs.length > 0 ? { deniedDirs } : {}),
    ...(fileCapture.captureTimeoutMs !== undefined ? { captureTimeoutMs: fileCapture.captureTimeoutMs } : {}),
    ...(fileCapture.maxFileBytes !== undefined ? { maxFileBytes: fileCapture.maxFileBytes } : {}),
    ...(fileCapture.maxTotalBytes !== undefined ? { maxTotalBytes: fileCapture.maxTotalBytes } : {}),
    ...(fileCapture.maxFiles !== undefined ? { maxFiles: fileCapture.maxFiles } : {})
  };
}

const DEFAULT_SESSION_IDLE_TTL = "3m";

export function sessionRetentionForWire(options: SessionCreateOptions): SessionRetentionPolicy {
  return {
    idleTtl: options.overrides?.idleTtl ?? DEFAULT_SESSION_IDLE_TTL
  };
}

export function sessionEnvironmentForWire(
  environment: SessionEnvironmentOptions | undefined
): PlatformEnvironmentInput | undefined {
  if (environment === undefined) {
    return undefined;
  }
  const { variables, secrets: _secrets, ...rest } = environment;
  void _secrets;
  const out: PlatformEnvironmentInput = {
    ...rest,
    ...(variables !== undefined ? { envVars: variables } : {})
  };
  return Object.keys(out).length === 0 ? undefined : out;
}

export function mergeMcpServers(
  inputs: readonly McpServer[],
  explicitSecrets: readonly PlatformMcpServerSecret[]
): {
  submissionMcpServers: ReadonlyArray<McpServerRef | { readonly kind: "workspace"; readonly id: string }>;
  mergedMcpSecrets: readonly PlatformMcpServerSecret[];
} {
  const submissionMcpServers: Array<McpServerRef | { readonly kind: "workspace"; readonly id: string }> = [];
  const secretByName = new Map<string, PlatformMcpServerSecret>();
  for (const secret of explicitSecrets) {
    secretByName.set(secret.name, secret);
  }
  for (let i = 0; i < inputs.length; i++) {
    const entry = inputs[i];
    if (!(entry instanceof McpServer)) {
      throw configError("aex", `mcpServers[${i}]`, `mcpServers[${i}] must be an McpServer instance`);
    }
    submissionMcpServers.push(entry.toSubmissionEntry());
    const secret = entry.toSecretEntry();
    if (secret) {
      const existing = secretByName.get(secret.name);
      if (existing && existing.url !== secret.url) {
        throw configError(
          "aex",
          `mcpServers[${i}].url`,
          `mcpServers[${i}].url conflicts with another MCP declaration`
        );
      }
      secretByName.set(secret.name, secret);
    }
  }
  return {
    submissionMcpServers,
    mergedMcpSecrets: Array.from(secretByName.values())
  };
}
