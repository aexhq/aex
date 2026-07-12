type FinishedRunObservation = Readonly<Record<string, unknown>>;

/**
 * Fail on the authoritative run result before a test performs checkpoint-backed
 * reads. This function is deliberately self-contained so its compiled source can
 * be injected into clean-install child scripts.
 */
export function requireSucceededRunBeforeFiles(
  label: string,
  result: FinishedRunObservation,
  knownSecrets: readonly unknown[] = [],
): void {
  if (result["ok"] === true && result["status"] === "succeeded") return;

  const safeText = (value: unknown): string | null => {
    if (typeof value !== "string" || value.length === 0) return null;
    let knownRedacted = value;
    for (const secret of knownSecrets) {
      if (typeof secret === "string" && secret.length > 0) {
        knownRedacted = knownRedacted.split(secret).join("[REDACTED]");
      }
    }
    return knownRedacted
      .replace(/\bsk-[A-Za-z0-9_-]{12,}\b/g, "[REDACTED]")
      .replace(/\b(?:apt|ant)_[A-Za-z0-9_-]{12,}\b/g, "[REDACTED]")
      .replace(/\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b/g, "[REDACTED]")
      .slice(0, 400);
  };
  const events = Array.isArray(result["events"])
    ? result["events"].filter((event): event is Record<string, unknown> =>
        typeof event === "object" && event !== null && !Array.isArray(event))
    : [];
  const terminal = [...events].reverse().find((event) => event["type"] === "RUN_ERROR");
  const terminalData = terminal !== undefined
    && typeof terminal["data"] === "object"
    && terminal["data"] !== null
    && !Array.isArray(terminal["data"])
    ? terminal["data"] as Record<string, unknown>
    : {};
  const run = typeof result["run"] === "object" && result["run"] !== null && !Array.isArray(result["run"])
    ? result["run"] as Record<string, unknown>
    : {};
  const diagnostic = {
    sessionId: safeText(result["sessionId"]),
    runId: safeText(run["runId"]),
    status: safeText(result["status"]),
    error: safeText(result["error"]),
    failureClass: safeText(terminalData["failureClass"]),
    failureMessage: safeText(terminalData["failureMessage"]),
    eventKinds: events.map((event) => safeText(event["type"])).filter((value) => value !== null),
  };
  throw new Error(`${label}: run ended before checkpoint-backed file reads: ${JSON.stringify(diagnostic)}`);
}

export function finishedRunReadinessSource(): string {
  return `const requireSucceededRunBeforeFiles = ${requireSucceededRunBeforeFiles.toString()};`;
}
