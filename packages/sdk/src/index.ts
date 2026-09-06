import { Brain, type BrainOptions } from "@aexhq/brain";

export { BrainError as AexError } from "@aexhq/brain";
export { agentloop, brainEnv, component, environment, hostEnv, tool } from "@aexhq/brain";
export type {
  PlacedAgentloop,
  BrainEnvOptions,
  Component,
  CreateSessionOptions,
  CustomProviderModel,
  Environment,
  KnownProviderId,
  KnownProviderModel,
  ModelContentBlock,
  ModelMessage,
  ModelResponse,
  ModelSelection,
  ModelStopReason,
  ModelUsage,
  OperationOptions,
  Outcome,
  Schema,
  SchemaInput,
  SchemaOutput,
  SessionEvent,
  SessionState,
  SessionStreamEvent,
  SessionTool,
  PlacedTool,
  ToolDefinition,
  VercelAiGatewayModel,
} from "@aexhq/brain";

const DEFAULT_API_URL = "https://api.aex.dev";

export interface AexOptions {
  apiKey: string;
  baseUrl?: string;
  fetch?: BrainOptions["fetch"];
  timeoutMs?: number;
}

export class Aex extends Brain {
  constructor(options: AexOptions) {
    if (options.apiKey.trim() === "") throw new TypeError("Aex apiKey cannot be empty");
    super({
      baseUrl: options.baseUrl ?? DEFAULT_API_URL,
      token: options.apiKey,
      ...(options.fetch === undefined ? {} : { fetch: options.fetch }),
      ...(options.timeoutMs === undefined ? {} : { timeoutMs: options.timeoutMs }),
    });
  }
}
