// Canonical committed run-terminal matcher.

export type TerminalOutcome = "succeeded" | "failed" | "timed_out" | "cancelled" | "interrupted";

export interface TerminalEvent {
  readonly type: "RUN_FINISHED" | "RUN_ERROR";
  readonly data: Record<string, unknown>;
}

interface ExpectTerminalOptions {
  readonly outcome: TerminalOutcome;
  readonly failureClass?: string;
  readonly context?: string;
}

/** Assert exactly one committed run terminal with the expected per-run data. */
export function expectTerminalEvent(
  events: ReadonlyArray<{ type: string; data: Record<string, unknown> }>,
  options: ExpectTerminalOptions
): TerminalEvent {
  const ctx = options.context ? ` [${options.context}]` : "";
  const dump = (): string => `\n  events=${events.map((event) => event.type).join(",")}`;
  const terminals = events.filter(
    (event): event is TerminalEvent => event.type === "RUN_FINISHED" || event.type === "RUN_ERROR"
  );
  if (terminals.length === 0) {
    throw new Error(`expectTerminalEvent${ctx}: no terminal (RUN_FINISHED|RUN_ERROR) in events${dump()}`);
  }
  if (terminals.length > 1) {
    throw new Error(`expectTerminalEvent${ctx}: ${terminals.length} terminal events; exactly one expected${dump()}`);
  }

  const terminal = terminals[0]!;
  const actualOutcome = terminal.data["outcome"];
  if (actualOutcome !== options.outcome) {
    throw new Error(
      `expectTerminalEvent${ctx}: outcome=${JSON.stringify(actualOutcome)} but expected ${JSON.stringify(options.outcome)}${dump()}`
    );
  }
  if (terminal.type === "RUN_ERROR" && actualOutcome !== "failed") {
    throw new Error(`expectTerminalEvent${ctx}: RUN_ERROR must carry outcome="failed"${dump()}`);
  }
  if (terminal.type === "RUN_FINISHED" && actualOutcome === "failed") {
    throw new Error(`expectTerminalEvent${ctx}: outcome="failed" must use RUN_ERROR${dump()}`);
  }
  if (options.failureClass !== undefined && terminal.data["failureClass"] !== options.failureClass) {
    throw new Error(
      `expectTerminalEvent${ctx}: failureClass=${JSON.stringify(terminal.data["failureClass"])} but expected ${JSON.stringify(options.failureClass)}${dump()}`
    );
  }
  const costUsd = terminal.data["costUsd"];
  if (typeof costUsd !== "number" || !Number.isFinite(costUsd) || costUsd < 0) {
    throw new Error(`expectTerminalEvent${ctx}: RUN terminal has invalid per-run costUsd${dump()}`);
  }
  if (!Array.isArray(terminal.data["providerUsage"])) {
    throw new Error(`expectTerminalEvent${ctx}: RUN terminal is missing per-run providerUsage${dump()}`);
  }
  if (terminal.type === "RUN_FINISHED") {
    const checkpoint = terminal.data["checkpoint"];
    if (
      !checkpoint ||
      typeof checkpoint !== "object" ||
      typeof (checkpoint as Record<string, unknown>)["checkpointId"] !== "string"
    ) {
      throw new Error(`expectTerminalEvent${ctx}: RUN_FINISHED is missing its committed checkpoint${dump()}`);
    }
  }
  return terminal;
}
