import { REDACTED, redactKnownValues } from "./live-diagnostics.js";

type LiveSessionStartObservation = Readonly<Record<string, unknown>>;
type KnownSecretValues = readonly (string | undefined)[];

const SECRET_SHAPES: readonly RegExp[] = [
  /\bsk-[A-Za-z0-9_-]{12,}\b/g,
  /\b(?:apt|ant)_[A-Za-z0-9_-]{12,}\b/g,
  /\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b/g,
  /\b(?:bearer|token|api[_-]?key|client[_-]?secret)["'\s:=]+[A-Za-z0-9_\-./+=]{12,}/gi
];

function diagnosticString(value: unknown, knownSecrets: KnownSecretValues): string | null {
  if (typeof value !== "string" || value.length === 0) return null;
  const knownRedacted = redactKnownValues(value, knownSecrets);
  return SECRET_SHAPES.reduce(
    (redacted, pattern) => redacted.replace(pattern, REDACTED),
    knownRedacted
  ).slice(0, 400);
}

function startErrorOf(
  result: LiveSessionStartObservation,
  knownSecrets: KnownSecretValues
): string | null {
  return (
    diagnosticString(result.runErr, knownSecrets) ??
    diagnosticString(result.startError, knownSecrets) ??
    diagnosticString(result.submitOrStartErr, knownSecrets)
  );
}

export function liveSessionStartDiagnostic(
  result: LiveSessionStartObservation,
  knownSecrets: KnownSecretValues = []
): string {
  return JSON.stringify({
    sessionId: diagnosticString(result.sessionId, knownSecrets),
    status: diagnosticString(result.status, knownSecrets),
    startError: startErrorOf(result, knownSecrets),
    errorMessage: diagnosticString(result.errorMessage, knownSecrets)
  });
}

/** Require evidence that client.start() returned a usable public session identity. */
export function requireStartedSessionIdentity(
  label: string,
  result: LiveSessionStartObservation,
  knownSecrets: KnownSecretValues = []
): string {
  const sessionId = typeof result.sessionId === "string" ? result.sessionId : null;
  const startError = startErrorOf(result, knownSecrets);
  if (sessionId === null || sessionId.trim().length === 0 || startError !== null) {
    throw new Error(
      `${label}: client.start() did not return a session identity ` +
      `(status=${JSON.stringify(diagnosticString(result.status, knownSecrets))}, startError=${JSON.stringify(startError)})`
    );
  }
  return sessionId;
}
