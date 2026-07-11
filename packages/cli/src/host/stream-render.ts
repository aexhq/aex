/**
 * Shared plumbing for `aex tail` / `aex inspect` (DX3): open the live
 * coordinator envelope stream and project each {@link AexEvent} to one
 * human-readable line.
 *
 * The stream reuses the SAME `@aexhq/contracts` primitives the SDK's
 * `session.events.streamEnvelopes()` does (`operations.getSessionCoordinatorTicket`
 * + `streamCoordinatorEvents`), so reconnect / replay-from-seq / idle-watchdog /
 * ping behaviour is byte-identical to the SDK — zero drift, no `@aexhq/sdk`
 * dependency. The WebSocket is dependency-injected via `io.webSocketFactory`
 * (real global `WebSocket` in production, a fake in tests).
 */
import {
  AEX_EVENT_SOURCES,
  AEX_EVENT_TYPES,
  channelOf,
  streamCoordinatorEvents,
  toAGUI,
  type AexStreamEvent,
  type HttpClient
} from "@aexhq/contracts";
import { operations } from "@aexhq/contracts/internal";
import type { CliIO } from "../internal.js";
import { suggest } from "./common.js";

export interface OpenEnvelopeStreamOptions {
  /** Replay cursor (events with sequence >= from). Default 0 = from start. */
  readonly from?: number;
  readonly signal?: AbortSignal;
  /** Stop at the durable terminal for this run rather than an earlier run. */
  readonly runId?: string;
  /** Non-secret diagnostics sink (wired to stderr under --debug). */
  readonly debug?: (line: string) => void;
}

/**
 * Open the live envelope stream for a session. Mirrors
 * `session.events.streamEnvelopes()` (`packages/sdk/src/client.ts`) but reads
 * the WS factory from {@link CliIO}. The caller MUST have verified
 * `io.webSocketFactory` is present.
 */
export async function* openEnvelopeStream(
  io: CliIO,
  http: HttpClient,
  sessionId: string,
  options: OpenEnvelopeStreamOptions = {}
): AsyncGenerator<AexStreamEvent, void, void> {
  const first = await operations.getSessionCoordinatorTicket(http, sessionId);
  if (options.debug) {
    let host = "(invalid)";
    try {
      host = new URL(first.wsUrl).host;
    } catch {
      /* keep placeholder */
    }
    // Never log the ticket value (a bearer secret) — host + expiry window only.
    options.debug(`ticket minted: host=${host} expiresInMs=${first.expiresAtMs - Date.now()} from=${options.from ?? 0}`);
  }
  yield* streamCoordinatorEvents({
    wsUrl: first.wsUrl,
    from: options.from ?? 0,
    fetchTicket: async () => (await operations.getSessionCoordinatorTicket(http, sessionId)).ticket,
    ...(options.runId ? {
      isTerminal: (event) =>
        event.runId === options.runId && (event.type === "RUN_FINISHED" || event.type === "RUN_ERROR")
    } : {}),
    ...(io.webSocketFactory ? { webSocketFactory: io.webSocketFactory } : {}),
    ...(options.signal ? { signal: options.signal } : {})
  });
}

export interface RenderOptions {
  /** Render `LOG`-channel records (hidden by default). */
  readonly logs?: boolean;
}

const PREVIEW_MAX = 120;

function clip(text: string, max = PREVIEW_MAX): string {
  const oneLine = text.replace(/\s+/g, " ").trim();
  return oneLine.length > max ? oneLine.slice(0, max - 1) + "…" : oneLine;
}

function str(v: unknown): string {
  return typeof v === "string" ? v : v === undefined || v === null ? "" : JSON.stringify(v);
}

/** Best-effort args preview for a TOOL_CALL_START envelope. */
function argsPreview(e: AexStreamEvent): string {
  const d = e.data as Record<string, unknown>;
  const candidate = d.input ?? d.args ?? d.arguments ?? d.params;
  if (candidate === undefined) return "";
  return clip(typeof candidate === "string" ? candidate : JSON.stringify(candidate));
}

/**
 * Project one envelope to a single human line, or `null` when it renders to
 * nothing (the caller skips). ASCII / light-unicode only; diagnostics-free.
 */
export function renderEnvelope(e: AexStreamEvent, options: RenderOptions = {}): string | null {
  if (channelOf(e) === "log" && !options.logs) return null;
  switch (e.type) {
    case "RUN_STARTED":
      return "▶ run started";
    case "TEXT_MESSAGE_CONTENT": {
      const delta = toAGUI(e).type === "TEXT_MESSAGE_CONTENT" ? (toAGUI(e) as { delta: string }).delta : "";
      return delta ? delta : null;
    }
    case "TOOL_CALL_START": {
      const agui = toAGUI(e);
      const name = agui.type === "TOOL_CALL_START" ? agui.toolCallName : str(e.data.name);
      const preview = argsPreview(e);
      return `· tool ${name || "(unnamed)"}(${preview})`;
    }
    case "TOOL_CALL_RESULT": {
      const name = str(e.data.name) || "result";
      const content = e.data.content;
      const ok = content === undefined || content === null ? "ok" : clip(str(content));
      return `  ← ${name} ${ok}`;
    }
    case "CUSTOM": {
      const label = e.message ?? str(e.data.name) ?? "custom";
      return `[aex] ${label}`;
    }
    case "LOG": {
      const level = e.level ?? "info";
      return `[${level}] ${e.message ?? str(e.data.message)}`;
    }
    case "RUN_FINISHED": {
      const outcome = str(e.data.outcome) || "succeeded";
      return outcome === "succeeded" ? "✓ run finished" : `✓ run finished (${outcome})`;
    }
    case "RUN_ERROR": {
      const agui = toAGUI(e);
      const message = agui.type === "RUN_ERROR" ? agui.message : (e.message ?? "run error");
      return `✗ run error: ${message}`;
    }
    default:
      return null;
  }
}

export interface ParsedFilters {
  /** Membership predicate, or undefined when no `--filter` was supplied. */
  readonly predicate?: (e: AexStreamEvent) => boolean;
  readonly error?: string;
}

/**
 * Parse `--filter` tokens (repeatable and/or comma-lists) into a predicate.
 * Each token is an AG-UI event TYPE (e.g. `TOOL_CALL_START`) or a coarse SOURCE
 * (e.g. `agent`). Types and sources compose as (type∈set ∧ source∈set), each
 * empty set meaning "any". An unknown token is an error with a "did you mean".
 */
export function parseFilters(tokens: readonly string[]): ParsedFilters {
  const flattened = tokens.flatMap((t) => t.split(",")).map((s) => s.trim()).filter(Boolean);
  if (flattened.length === 0) return {};
  const types = new Set<string>();
  const sources = new Set<string>();
  for (const tok of flattened) {
    if ((AEX_EVENT_TYPES as readonly string[]).includes(tok)) {
      types.add(tok);
    } else if ((AEX_EVENT_SOURCES as readonly string[]).includes(tok)) {
      sources.add(tok);
    } else {
      const hint = suggest(tok, [...AEX_EVENT_TYPES, ...AEX_EVENT_SOURCES]);
      return {
        error:
          `unknown --filter token "${tok}"${hint ? `; did you mean "${hint}"?` : ""} ` +
          `(types: ${AEX_EVENT_TYPES.join(",")}; sources: ${AEX_EVENT_SOURCES.join(",")})`
      };
    }
  }
  const predicate = (e: AexStreamEvent): boolean =>
    (types.size === 0 || types.has(e.type)) && (sources.size === 0 || sources.has(e.source));
  return { predicate };
}
