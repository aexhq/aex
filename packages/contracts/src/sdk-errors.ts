import { redactSecrets } from "./sdk-secrets.js";

export type AexErrorCode =
  | "RUN_CONFIG_INVALID"
  | "CREDENTIAL_INVALID"
  | "PROVIDER_ERROR"
  | "RUN_STATE_ERROR"
  | "CLEANUP_ERROR"
  | "RUNTIME_UNSUPPORTED"
  | "API_ERROR";

export class AexError extends Error {
  readonly code: AexErrorCode;
  readonly details?: unknown;

  constructor(code: AexErrorCode, message: string, details?: unknown) {
    super(redactSecrets(message));
    this.name = this.constructor.name;
    this.code = code;
    this.details = details === undefined ? undefined : redactSecrets(details);
  }
}

export class RunConfigValidationError extends AexError {
  constructor(message: string, details?: unknown) {
    super("RUN_CONFIG_INVALID", message, details);
  }
}

export class CredentialValidationError extends AexError {
  constructor(message: string, details?: unknown) {
    super("CREDENTIAL_INVALID", message, details);
  }
}

export class ProviderError extends AexError {
  readonly status: number | undefined;

  constructor(message: string, options: { status?: number; details?: unknown } = {}) {
    super("PROVIDER_ERROR", message, options.details);
    this.status = options.status;
  }
}

export class RunStateError extends AexError {
  constructor(message: string, details?: unknown) {
    super("RUN_STATE_ERROR", message, details);
  }
}

export class CleanupError extends AexError {
  constructor(message: string, details?: unknown) {
    super("CLEANUP_ERROR", message, details);
  }
}

/**
 * Thrown by SDK and CLI operations when the dashboard BFF returns a non-2xx
 * response. Carries the HTTP status and parsed body for the caller to inspect.
 */
export class AexApiError extends AexError {
  readonly status: number;
  readonly body: unknown;

  constructor(status: number, message: string, body: unknown) {
    super("API_ERROR", message, body);
    this.status = status;
    this.body = redactSecrets(body);
  }
}
