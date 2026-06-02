import { redactSecrets } from "./sdk-secrets.js";

export type AntpathErrorCode =
  | "RUN_CONFIG_INVALID"
  | "CREDENTIAL_INVALID"
  | "PROVIDER_ERROR"
  | "RUN_STATE_ERROR"
  | "CLEANUP_ERROR"
  | "RUNTIME_UNSUPPORTED"
  | "API_ERROR";

export class AntpathError extends Error {
  readonly code: AntpathErrorCode;
  readonly details?: unknown;

  constructor(code: AntpathErrorCode, message: string, details?: unknown) {
    super(redactSecrets(message));
    this.name = this.constructor.name;
    this.code = code;
    this.details = details === undefined ? undefined : redactSecrets(details);
  }
}

export class RunConfigValidationError extends AntpathError {
  constructor(message: string, details?: unknown) {
    super("RUN_CONFIG_INVALID", message, details);
  }
}

export class CredentialValidationError extends AntpathError {
  constructor(message: string, details?: unknown) {
    super("CREDENTIAL_INVALID", message, details);
  }
}

export class ProviderError extends AntpathError {
  readonly status: number | undefined;

  constructor(message: string, options: { status?: number; details?: unknown } = {}) {
    super("PROVIDER_ERROR", message, options.details);
    this.status = options.status;
  }
}

export class RunStateError extends AntpathError {
  constructor(message: string, details?: unknown) {
    super("RUN_STATE_ERROR", message, details);
  }
}

export class CleanupError extends AntpathError {
  constructor(message: string, details?: unknown) {
    super("CLEANUP_ERROR", message, details);
  }
}

/**
 * Thrown by SDK and CLI operations when the dashboard BFF returns a non-2xx
 * response. Carries the HTTP status and parsed body for the caller to inspect.
 */
export class AntpathApiError extends AntpathError {
  readonly status: number;
  readonly body: unknown;

  constructor(status: number, message: string, body: unknown) {
    super("API_ERROR", message, body);
    this.status = status;
    this.body = redactSecrets(body);
  }
}
