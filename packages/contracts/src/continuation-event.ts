/**
 * Dependency-free trace carrier used at continuation boundaries.
 *
 * This contract is internal to the aex runtime packages. It deliberately
 * lives in the public contracts package so every runtime consumes one wire
 * shape without importing a hosted-platform implementation package.
 */
export interface W3CTraceContext {
  readonly traceparent: string;
  readonly tracestate?: string;
}

export const CONTINUATION_KINDS = [
  "user_message",
  "tool_result",
  "scheduled_wake",
  "subagent_done"
] as const;

export type ContinuationKind = (typeof CONTINUATION_KINDS)[number];
export type ContinuationToken = string;

/** Durable continuation payload shared by the outbox and queue transports. */
export interface ContinuationEvent {
  readonly kind: ContinuationKind;
  readonly token: ContinuationToken;
  readonly agentId: string;
  readonly sessionId: string;
  readonly observedEpoch?: number;
  readonly turnSeq?: number;
  readonly callId?: string;
  readonly childAgentId?: string;
  readonly wakeId?: string;
  /** Semantic identity of the next planned action; excluded from token identity. */
  readonly decisionFp?: string;
  readonly requeueAttempt?: number;
  /** W3C context propagated across the asynchronous continuation boundary. */
  readonly traceContext?: W3CTraceContext;
}
