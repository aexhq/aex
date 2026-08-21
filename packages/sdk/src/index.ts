import { Sessions } from "./session.js";
import type { Fetch } from "./transport.js";
import { Transport } from "./transport.js";
import type { WebSocketFactory } from "@aexhq/brain";

export {
  AbortError,
  AexError,
  OutputRefusalError,
  OutputSchemaError,
  OutputValidationError,
  SessionError,
} from "./errors.js";
export type { AexErrorOptions, OutputValidationIssue } from "./errors.js";
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
export {
  SandboxFiles,
  SessionChild,
  SessionChildren,
  SessionSandbox,
  SessionStorage,
} from "./resources.js";
export type {
  BinarySource,
  ChildSummary,
  IdempotentOperationOptions,
  OperationOptions,
  PageOptions,
  SandboxFile,
  SandboxFileOptions,
  SandboxFilePage,
  SandboxFilePageOptions,
  SandboxStatus,
  StorageObject,
  StoragePage,
  StreamingUploadSource,
  UploadSource,
} from "./resources.js";
export type { EventOptions } from "./transport.js";
export { tool } from "@aexhq/brain";
export type {
  ClientToolOptions,
  Tool,
  ToolBuilder,
  ToolContract,
  ToolContext,
  ToolHandler,
  ServerToolOptions,
  NetworkDestination,
  NetworkPolicy,
  WebSocketFactory,
} from "@aexhq/brain";

const DEFAULT_API_URL = "https://api.aex.dev";

export interface AexOptions {
  apiKey: string;
  baseUrl?: string;
  fetch?: Fetch;
  webSocketFactory?: WebSocketFactory;
  /** Stable tenant-scoped identity for this exact customer-application runner. */
  client?: { id: string };
}

export class Aex {
  readonly sessions: Sessions;

  constructor(options: AexOptions) {
    if (options.apiKey.trim() === "") throw new TypeError("Aex apiKey cannot be empty");
    const fetchImplementation = options.fetch ?? globalThis.fetch?.bind(globalThis);
    if (fetchImplementation === undefined) {
      throw new TypeError("This runtime does not provide fetch; pass a fetch implementation to Aex");
    }
    if (
      options.client !== undefined &&
      !/^[A-Za-z0-9_.:-]{1,128}$/u.test(options.client.id)
    ) {
      throw new TypeError(
        "Aex client.id must contain 1 through 128 letters, digits, dots, colons, underscores, or hyphens",
      );
    }
    const transport = new Transport(options.apiKey, options.baseUrl ?? DEFAULT_API_URL, fetchImplementation);
    const webSocketFactory = options.webSocketFactory ??
      (globalThis.WebSocket === undefined
        ? undefined
        : (request) => new globalThis.WebSocket(request.url, request.protocol));
    this.sessions = new Sessions(transport, webSocketFactory, options.client?.id);
  }

  /** Stop customer-app execution permanently; this Aex instance cannot create another session. */
  close(): void {
    this.sessions.close();
  }
}
