import type { RuntimeKind, RuntimeSize } from "@aexhq/contracts";
import type { StartArguments } from "./start-arguments.js";
import type { StartAttachments } from "./start-attachments.js";
import type { ResolvedStartConfig } from "./start-config.js";
import {
  toCliSessionEnvironment,
  type CliSessionSubmitOptions
} from "./start-submit.js";

const DEFAULT_SESSION_IDLE_TTL = "3m";

/** Purely combine resolved start stages into the existing session-create input. */
export function buildStartSubmission(
  args: StartArguments,
  resolved: ResolvedStartConfig,
  attachments: StartAttachments
): CliSessionSubmitOptions {
  const environment = toCliSessionEnvironment(resolved.environment);
  const runtimeSize = (args.runtimeSize as RuntimeSize | null) ?? resolved.configRuntimeSize;
  const runtimeKind = args.runtimeKind as RuntimeKind | null;
  const runtime =
    runtimeKind || runtimeSize
      ? { ...(runtimeKind ? { kind: runtimeKind } : {}), ...(runtimeSize ? { size: runtimeSize } : {}) }
      : undefined;
  const timeout = args.sessionTimeout ?? resolved.configTimeout;

  return {
    message: resolved.message,
    provider: resolved.provider,
    model: resolved.model,
    ...(resolved.system ? { system: resolved.system } : {}),
    ...(attachments.skills.length > 0 ? { skills: attachments.skills } : {}),
    ...(attachments.tools.length > 0 ? { tools: attachments.tools } : {}),
    ...(attachments.instructions.length > 0 ? { instructions: attachments.instructions } : {}),
    ...(attachments.files.length > 0 ? { files: attachments.files } : {}),
    ...(resolved.mcpServers.length > 0 ? { mcpServers: resolved.mcpServers } : {}),
    ...(resolved.metadata ? { metadata: resolved.metadata } : {}),
    apiKeys: args.providerApiKeys,
    ...(environment ? { environment } : {}),
    ...(runtime ? { runtime } : {}),
    overrides: {
      idleTtl: DEFAULT_SESSION_IDLE_TTL,
      ...(timeout ? { timeout } : {})
    },
    ...(args.webhookUrl ? { webhook: { url: args.webhookUrl } } : {}),
    ...(args.idempotencyKey ? { idempotencyKey: args.idempotencyKey } : {})
  };
}
