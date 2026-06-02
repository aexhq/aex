// Pins the SDK's structured-error contract: AntpathError (or other
// named subclasses), never a bare `Error`. Used by failure-case tests
// (b1, b2, b3 in live-sdk-outputs-and-failures.test.ts).
//
// The pre-Phase-1 shape was `if (errorClass === "Error" || errorClass === null) throw`,
// which let an `errorClass: null` slip through silently. This matcher
// makes the contract explicit and the failure mode obvious.

export interface StructuredError {
  readonly errorClass: string | null | undefined;
  readonly errorCode?: string | null | undefined;
  readonly errorMessage?: string | null | undefined;
}

interface ExpectStructuredErrorOptions {
  /** Allow-list of acceptable error class names. Default: must include "AntpathError". */
  readonly classes?: ReadonlyArray<string>;
  /** Optional substring (case-insensitive) the error message MUST contain. */
  readonly messageIncludes?: string;
  /** Optional context string interpolated into failure messages. */
  readonly context?: string;
}

/**
 * Assert that a captured error is structured (a named error class, not
 * a bare Error or null) and optionally check its message contains a
 * required substring. The default class allow-list is ["AntpathError"]
 * which matches the SDK's public error surface.
 */
export function expectStructuredError(
  err: StructuredError,
  options: ExpectStructuredErrorOptions = {}
): void {
  const ctx = options.context ? ` [${options.context}]` : "";
  const acceptable = new Set(options.classes ?? ["AntpathError"]);
  const dump = (): string =>
    `\n  errorClass=${err.errorClass} errorCode=${err.errorCode ?? null} errorMessage=${err.errorMessage ?? null}`;

  // Bare Error / null / undefined / "Object" are all considered
  // unstructured — the customer can't catch on type.
  if (
    err.errorClass === null ||
    err.errorClass === undefined ||
    err.errorClass === "Error" ||
    err.errorClass === "Object"
  ) {
    throw new Error(
      `expectStructuredError${ctx}: error class is not structured (got ${JSON.stringify(err.errorClass)})${dump()}`
    );
  }
  if (!acceptable.has(err.errorClass)) {
    throw new Error(
      `expectStructuredError${ctx}: error class ${JSON.stringify(err.errorClass)} not in allow-list ${JSON.stringify([...acceptable])}${dump()}`
    );
  }
  if (options.messageIncludes !== undefined) {
    const msg = (err.errorMessage ?? "").toLowerCase();
    if (!msg.includes(options.messageIncludes.toLowerCase())) {
      throw new Error(
        `expectStructuredError${ctx}: error message does not include ${JSON.stringify(options.messageIncludes)}${dump()}`
      );
    }
  }
}
