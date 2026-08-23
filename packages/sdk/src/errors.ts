import type { ApiError, ApiErrorCode } from "@aexhq/session-protocol/session";

/** One Aex trusted-output validation failure, expressed as a JSON Pointer. */
export interface OutputValidationIssue {
  path: string;
  message: string;
  keyword?: string;
}

export interface AexErrorOptions {
  code?: ApiErrorCode | undefined;
  status?: number | undefined;
  param?: string | undefined;
  requestId?: string | undefined;
  cause?: unknown;
}

export class AexError extends Error {
  readonly code: ApiErrorCode | undefined;
  readonly status: number | undefined;
  readonly param: string | undefined;
  readonly requestId: string | undefined;

  constructor(message: string, options: AexErrorOptions = {}) {
    super(message, options.cause === undefined ? undefined : { cause: options.cause });
    this.name = "AexError";
    this.code = options.code;
    this.status = options.status;
    this.param = options.param;
    this.requestId = options.requestId;
  }
}

export class OutputSchemaError extends AexError {
  constructor(message: string, options: AexErrorOptions = {}) {
    super(message, { ...options, code: "output_schema_error" });
    this.name = "OutputSchemaError";
  }
}

export class OutputRefusalError extends AexError {
  constructor(message: string, options: AexErrorOptions = {}) {
    super(message, { ...options, code: "output_refused" });
    this.name = "OutputRefusalError";
  }
}

export class OutputValidationError extends AexError {
  readonly issues: readonly OutputValidationIssue[];

  constructor(
    message: string,
    issues: readonly OutputValidationIssue[] = [],
    options: AexErrorOptions = {},
  ) {
    super(message, { ...options, code: "output_validation_error" });
    this.name = "OutputValidationError";
    this.issues = issues;
  }
}

export class SessionError extends AexError {
  constructor(message: string, options: AexErrorOptions = {}) {
    super(message, options);
    this.name = "SessionError";
  }
}

export class AbortError extends AexError {
  constructor(message = "The operation was cancelled", options: AexErrorOptions = {}) {
    super(message, { ...options, code: "cancelled" });
    this.name = "AbortError";
  }
}

export function errorFromApi(
  error: ApiError,
  status?: number,
  issues: readonly OutputValidationIssue[] = [],
): AexError {
  const options: AexErrorOptions = {
    code: error.code,
    status,
    param: error.param,
    requestId: error.request_id,
  };
  switch (error.code) {
    case "output_schema_error":
      return new OutputSchemaError(error.message, options);
    case "output_refused":
      return new OutputRefusalError(error.message, options);
    case "output_validation_error":
      return new OutputValidationError(error.message, issues, options);
    case "cancelled":
      return new AbortError(error.message, options);
    default:
      return new SessionError(error.message, options);
  }
}

export function abortError(cause?: unknown): AbortError {
  return new AbortError("The operation was cancelled", { cause });
}
