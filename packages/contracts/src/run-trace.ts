import type { UsageSummary } from "./runtime-types.js";

export interface ToolCallTrace {
  readonly id: string;
  readonly name: string;
  readonly args: Readonly<Record<string, unknown>>;
  readonly messageId?: string;
  readonly startSeq?: number;
  readonly startedAt?: string;
  readonly result?: ToolCallResult;
  readonly durationMs?: number;
}

export interface ToolCallResult {
  readonly isError: boolean;
  readonly content: unknown;
  readonly seq?: number;
  readonly recordedAt?: string;
}

export interface AssistantTextEntry {
  readonly text: string;
  readonly messageId?: string;
  readonly seq?: number;
  readonly recordedAt?: string;
}

export interface RunTrace {
  readonly toolCalls: readonly ToolCallTrace[];
  readonly usage: UsageSummary;
  readonly text: readonly AssistantTextEntry[];
}
