import { BrainClient, BrainError, type BrainOptions } from "@aexhq/brain";

import { Sessions } from "./session.js";

export { BrainError as AexError } from "@aexhq/brain";
export { Session, Sessions } from "./session.js";
export type { RequestOptions } from "./session.js";
export type {
  AgentloopAdmission,
  CreateSessionRequest,
  EnvironmentRequirement,
  EventPage,
  SessionEvent,
  SessionList,
  ToolBinding,
  ToolDefinition,
} from "@aexhq/brain";

const DEFAULT_API_URL = "https://api.aex.dev";

export interface AexOptions {
  apiKey: string;
  baseUrl?: string;
  fetch?: BrainOptions["fetch"];
  timeoutMs?: number;
}

export class Aex {
  readonly brain: BrainClient;
  readonly sessions: Sessions;

  constructor(options: AexOptions) {
    if (options.apiKey.trim() === "") throw new TypeError("Aex apiKey cannot be empty");
    this.brain = new BrainClient({
      baseUrl: options.baseUrl ?? DEFAULT_API_URL,
      apiKey: options.apiKey,
      ...(options.fetch === undefined ? {} : { fetch: options.fetch }),
      ...(options.timeoutMs === undefined ? {} : { timeoutMs: options.timeoutMs }),
    });
    this.sessions = new Sessions(this.brain);
  }
}

export { BrainError };
