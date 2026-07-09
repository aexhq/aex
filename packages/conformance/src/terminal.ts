// Canonical terminal-event matcher. Replaces every inline
// `if (terminal) expect(terminal.data["reason"]).not.toBe("error")` etc.
// across the live tests.
//
// Contract:
//   - The events array MUST contain exactly one sessiontime_terminal.
//   - The terminal MUST carry `reason` of the expected value (no
//     undefined-skip — if the field can be absent, that's a different
//     test, not a tolerance of THIS matcher).
//   - Optionally pin `failureClass` for error-flavoured terminals.

export type TerminalReason = "complete" | "error" | "cancelled" | "max_tokens" | "timed_out";

export interface TerminalEvent {
  readonly type: string;
  readonly data: Record<string, unknown>;
}

interface ExpectTerminalOptions {
  /** Required terminal reason. No default — be explicit at the call site. */
  readonly reason: TerminalReason;
  /** Required failureClass when `reason === "error"`. Optional otherwise. */
  readonly failureClass?: string;
  /** Optional context string interpolated into failure messages. */
  readonly context?: string;
}

/**
 * Assert that `events` contains a single runtime_terminal matching the
 * expected reason (and failureClass when applicable). Returns the
 * matched terminal event so the caller can drill further if needed.
 *
 * Throws a structured Error on any mismatch — never returns a "maybe".
 */
export function expectTerminalEvent(
  events: ReadonlyArray<{ type: string; data: Record<string, unknown> }>,
  options: ExpectTerminalOptions
): TerminalEvent {
  const ctx = options.context ? ` [${options.context}]` : "";
  const dump = (): string => `\n  events=${events.map((e) => e.type).join(",")}`;

  const terminals = events.filter((e) => e.type === "TURN_FINISHED" || e.type === "TURN_ERROR");
  if (terminals.length === 0) {
    throw new Error(`expectTerminalEvent${ctx}: no terminal (TURN_FINISHED|TURN_ERROR) in events${dump()}`);
  }
  if (terminals.length > 1) {
    throw new Error(
      `expectTerminalEvent${ctx}: ${terminals.length} terminal events — exactly one expected${dump()}`
    );
  }
  const terminal = terminals[0]!;
  const actualReason = terminal.data["reason"];
  if (actualReason !== options.reason) {
    throw new Error(
      `expectTerminalEvent${ctx}: reason=${JSON.stringify(actualReason)} but expected ${JSON.stringify(options.reason)}${dump()}`
    );
  }
  if (options.reason === "error" && options.failureClass !== undefined) {
    const actualClass = terminal.data["failureClass"];
    if (actualClass !== options.failureClass) {
      throw new Error(
        `expectTerminalEvent${ctx}: failureClass=${JSON.stringify(actualClass)} but expected ${JSON.stringify(options.failureClass)}${dump()}`
      );
    }
  }
  return terminal;
}
