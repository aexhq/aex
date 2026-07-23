/**
 * Widening seams for runtime-validator tests (B5).
 *
 * The SDK's submit-boundary validators exist to guard against UNTYPED JS
 * callers, so their tests must hand payloads BEYOND the declared option types
 * to APIs typed for valid ones — malformed payloads the validator must reject,
 * and open-map payloads (future provider keys, caller-defined metadata) it
 * must accept. Each helper below is the single, documented place where a plain
 * record is widened into the option contract it exceeds — test files
 * themselves stay free of `as never` / `as unknown as`. Never use these
 * outside validator-boundary tests.
 */
import type {
  SessionCreateOptions,
  SessionSendOptions,
  SessionStartOptions,
  StartSessionOptions
} from "../../src/index.js";

function widen<T>(record: Readonly<Record<string, unknown>> | undefined): T {
  const unvalidated: unknown = record;
  return unvalidated as T;
}

/** A deliberately-invalid `aex.sessions.create` payload (`undefined` models a missing-options caller). */
export function unvalidatedCreateOptions(record: Readonly<Record<string, unknown>> | undefined): SessionCreateOptions {
  return widen<SessionCreateOptions>(record);
}

/** A deliberately-invalid `Aex.start` first argument. */
export function unvalidatedStartOptions(record: Readonly<Record<string, unknown>>): SessionStartOptions {
  return widen<SessionStartOptions>(record);
}

/** A deliberately-invalid `Aex.start` second (control) argument. */
export function unvalidatedStartControls(record: Readonly<Record<string, unknown>>): StartSessionOptions {
  return widen<StartSessionOptions>(record);
}

/** A deliberately-invalid `session.messages.send` options argument. */
export function unvalidatedSendOptions(record: Readonly<Record<string, unknown>>): SessionSendOptions {
  return widen<SessionSendOptions>(record);
}
