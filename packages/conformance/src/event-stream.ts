// Bracketing matcher for the unified event stream (AG-UI envelope vocabulary).
//
// Pins the ordering signal that `toContain` lost:
//
//   - RUN_STARTED MUST appear.
//   - A terminal (RUN_FINISHED or RUN_ERROR) MUST appear.
//   - Every "signal-bearing" event (TEXT_MESSAGE_CONTENT, TOOL_CALL_START,
//     TOOL_CALL_RESULT) MUST come AFTER RUN_STARTED and BEFORE the terminal.
//   - Bookkeeping events (CUSTOM — antpath.notification / antpath.* — and any
//     other non-signal type) may appear anywhere, including after the
//     terminal: they are the legitimate "batch carries an extra status event
//     after idle" case, expressed as data instead of a tolerance.

export interface EventStreamShape {
  /** Types the stream may carry after the terminal. Defaults to bookkeeping (CUSTOM). */
  readonly allowAfterTerminal?: ReadonlyArray<string>;
  /** Optional context string interpolated into failure messages. */
  readonly context?: string;
}

const SIGNAL_KINDS = new Set(["TEXT_MESSAGE_CONTENT", "TOOL_CALL_START", "TOOL_CALL_RESULT"]);
const DEFAULT_ALLOW_AFTER_TERMINAL = ["CUSTOM"];

/**
 * Validate the bracketing of a unified event stream. Throws on any
 * violation — never returns a "maybe". On success, returns the indices of
 * the start/terminal so the caller can slice further if needed.
 */
export function expectEventStream(
  events: ReadonlyArray<{ type: string }>,
  options: EventStreamShape = {}
): { readonly startedIdx: number; readonly terminalIdx: number } {
  const ctx = options.context ? ` [${options.context}]` : "";
  const kinds = events.map((e) => e.type);
  const dump = (): string => `\n  kinds=${kinds.join(",")}`;
  const allowAfter = new Set(options.allowAfterTerminal ?? DEFAULT_ALLOW_AFTER_TERMINAL);

  const startedIdx = kinds.indexOf("RUN_STARTED");
  if (startedIdx === -1) {
    throw new Error(`expectEventStream${ctx}: RUN_STARTED missing${dump()}`);
  }
  // The terminal is RUN_FINISHED (normal) or RUN_ERROR (error). Take the last
  // of either — the coordinator is the seq authority, terminal is last.
  const terminalIdx = Math.max(kinds.lastIndexOf("RUN_FINISHED"), kinds.lastIndexOf("RUN_ERROR"));
  if (terminalIdx === -1) {
    throw new Error(`expectEventStream${ctx}: terminal (RUN_FINISHED|RUN_ERROR) missing${dump()}`);
  }
  if (terminalIdx < startedIdx) {
    throw new Error(
      `expectEventStream${ctx}: terminal at idx ${terminalIdx} precedes RUN_STARTED at idx ${startedIdx}${dump()}`
    );
  }
  // Every signal event must be inside [startedIdx, terminalIdx] UNLESS
  // the caller has explicitly allow-listed that type post-terminal.
  for (let i = 0; i < events.length; i++) {
    const kind = kinds[i]!;
    if (!SIGNAL_KINDS.has(kind)) continue;
    if (i < startedIdx) {
      throw new Error(`expectEventStream${ctx}: signal event ${kind} at idx ${i} precedes RUN_STARTED${dump()}`);
    }
    if (i > terminalIdx && !allowAfter.has(kind)) {
      throw new Error(`expectEventStream${ctx}: signal event ${kind} at idx ${i} comes after the terminal${dump()}`);
    }
  }
  // Every non-signal event after terminal must be in the allow-list too.
  for (let i = terminalIdx + 1; i < events.length; i++) {
    const kind = kinds[i]!;
    if (!allowAfter.has(kind)) {
      throw new Error(
        `expectEventStream${ctx}: event ${kind} at idx ${i} follows the terminal but is not in allowAfterTerminal (${[...allowAfter].join(",")})${dump()}`
      );
    }
  }
  return { startedIdx, terminalIdx };
}
