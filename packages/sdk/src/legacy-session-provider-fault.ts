import type { DebugSink, ProviderFault, Session } from "@aexhq/contracts";

const LEGACY_HTTP_FAILURE =
  /^llm provider unavailable \(HTTP (\d{3})\) — throttled or overloaded; request was not replayed automatically(?:: .+)?$/;
const LEGACY_STREAM_FAILURE =
  /^llm provider stream error \((rate_limit_error|overloaded_error|api_error|timeout_error)\) — throttled or overloaded mid-stream; retry later$/;

/**
 * Temporary field-absent compatibility bridge for pre-ProviderFault sessions.
 * It recognizes only the historical failure class/templates and never carries
 * the free-text message into the returned public fault.
 */
export function legacySessionProviderFault(
  session: Session,
  debug?: DebugSink
): ProviderFault | undefined {
  // A present canonical field always wins, including a present non-throttle.
  if (Object.hasOwn(session, "providerFault")) return undefined;

  const message = typeof session.errorMessage === "string" ? session.errorMessage : undefined;
  const httpMatch = message?.match(LEGACY_HTTP_FAILURE);
  const streamMatch = message?.match(LEGACY_STREAM_FAILURE);

  let fault: ProviderFault | undefined;
  if (httpMatch) {
    const status = Number(httpMatch[1]);
    const kind = status === 429 ? "rate_limit" : status === 529 ? "overloaded" : "unavailable";
    fault = { kind, status };
  } else if (streamMatch) {
    const errorType = streamMatch[1];
    const kind = errorType === "rate_limit_error"
      ? "rate_limit"
      : errorType === "overloaded_error"
        ? "overloaded"
        : "unavailable";
    fault = { kind };
  } else if (session.failureClass === "transient-provider") {
    fault = { kind: "unavailable" };
  }

  if (fault !== undefined) {
    debug?.(`[aex] legacy_provider_fault_fallback kind=${fault.kind} source=session`);
  }
  return fault;
}
