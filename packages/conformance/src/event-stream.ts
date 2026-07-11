// Bracketing matcher for the canonical AG-UI run event stream.

export interface EventStreamShape {
  /** Optional context string interpolated into failure messages. */
  readonly context?: string;
}

const SIGNAL_KINDS = new Set(["TEXT_MESSAGE_CONTENT", "TOOL_CALL_START", "TOOL_CALL_RESULT"]);

/**
 * Validate one run's event ordering. RUN_FINISHED/RUN_ERROR is the committed
 * consistency barrier, so no event may follow it.
 */
export function expectEventStream(
  events: ReadonlyArray<{ type: string }>,
  options: EventStreamShape = {}
): { readonly startedIdx: number; readonly terminalIdx: number } {
  const ctx = options.context ? ` [${options.context}]` : "";
  const kinds = events.map((event) => event.type);
  const dump = (): string => `\n  kinds=${kinds.join(",")}`;

  const startedIdx = kinds.indexOf("RUN_STARTED");
  if (startedIdx === -1) {
    throw new Error(`expectEventStream${ctx}: RUN_STARTED missing${dump()}`);
  }
  const terminalIdx = Math.max(kinds.lastIndexOf("RUN_FINISHED"), kinds.lastIndexOf("RUN_ERROR"));
  if (terminalIdx === -1) {
    throw new Error(`expectEventStream${ctx}: terminal (RUN_FINISHED|RUN_ERROR) missing${dump()}`);
  }
  if (terminalIdx < startedIdx) {
    throw new Error(
      `expectEventStream${ctx}: terminal at idx ${terminalIdx} precedes RUN_STARTED at idx ${startedIdx}${dump()}`
    );
  }
  if (terminalIdx !== events.length - 1) {
    throw new Error(
      `expectEventStream${ctx}: event ${kinds[terminalIdx + 1]} at idx ${terminalIdx + 1} follows the committed terminal${dump()}`
    );
  }
  for (let index = 0; index < events.length; index += 1) {
    const kind = kinds[index]!;
    if (!SIGNAL_KINDS.has(kind)) continue;
    if (index < startedIdx) {
      throw new Error(`expectEventStream${ctx}: signal event ${kind} at idx ${index} precedes RUN_STARTED${dump()}`);
    }
    if (index > terminalIdx) {
      throw new Error(`expectEventStream${ctx}: signal event ${kind} at idx ${index} follows the terminal${dump()}`);
    }
  }
  return { startedIdx, terminalIdx };
}
