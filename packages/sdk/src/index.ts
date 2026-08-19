import { Sessions } from "./session.js";
import type { Fetch } from "./transport.js";
import { Transport } from "./transport.js";

export {
  AbortError,
  AexError,
  OutputRefusalError,
  OutputSchemaError,
  OutputValidationError,
  SessionError,
} from "./errors.js";
export type { AexErrorOptions } from "./errors.js";
export {
  Session,
  Sessions,
} from "./session.js";
export type {
  CreateSessionOptions,
  ListSessionsOptions,
  ModelSummary,
  ModelOptions,
  OutputOptions,
  RequestOptions,
  SessionInput,
  SessionList,
  SessionSummary,
} from "./session.js";
export type { EventOptions } from "./transport.js";

const DEFAULT_API_URL = "https://api.aex.dev";

export interface AexOptions {
  apiKey: string;
  baseUrl?: string;
  fetch?: Fetch;
}

export class Aex {
  readonly sessions: Sessions;

  constructor(options: AexOptions) {
    if (options.apiKey.trim() === "") throw new TypeError("AEX apiKey cannot be empty");
    const fetchImplementation = options.fetch ?? globalThis.fetch?.bind(globalThis);
    if (fetchImplementation === undefined) {
      throw new TypeError("This runtime does not provide fetch; pass a fetch implementation to Aex");
    }
    const transport = new Transport(options.apiKey, options.baseUrl ?? DEFAULT_API_URL, fetchImplementation);
    this.sessions = new Sessions(transport);
  }
}
