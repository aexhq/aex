/**
 * Tiny SSE (text/event-stream) parser used by the SDK / CLI to consume
 * the dashboard's `/api/sessions/:id/events/stream` endpoint without
 * pulling in a DOM `EventSource` dependency. Node's `fetch` returns a
 * web `ReadableStream<Uint8Array>` body; this parser turns that into
 * an async iterable of `SseFrame`s.
 *
 * The wire grammar follows the WHATWG HTML spec § Server-sent events:
 *
 *   - Lines are separated by `\n`, `\r`, or `\r\n`.
 *   - A `:` prefix marks a comment — ignored.
 *   - `field: value` (or `field:value`, with no space) — leading space
 *     after the colon is stripped.
 *   - Recognised fields: `event`, `data`, `id`, `retry`.
 *   - Multiple `data:` lines join with `\n`.
 *   - A blank line dispatches the current frame and resets the buffer.
 *   - `event:` defaults to `"message"` when absent.
 *
 * The parser is deliberately a free function so it can be unit-tested
 * against synthesized chunk boundaries (one byte at a time, mid-frame,
 * across UTF-8 multi-byte sequences) without spinning up a real
 * connection.
 */

export interface SseFrame {
  readonly id?: string;
  readonly event: string;
  readonly data: string;
  readonly retry?: number;
}

/**
 * Stateful parser: feed in chunks (in any size), pull out completed
 * frames. Designed for `for await` consumption — see `iterateSse`.
 */
export class SseParser {
  #buffer = "";
  #decoder = new TextDecoder("utf-8");
  #eventType = "";
  #dataLines: string[] = [];
  #lastEventId: string | undefined;
  #retry: number | undefined;

  /**
   * Push a chunk of raw bytes through the parser. Returns any frames
   * that completed during this chunk. Chunks may contain zero, one,
   * or many frame boundaries — the parser handles partial trailing
   * data internally.
   */
  pushBytes(chunk: Uint8Array): readonly SseFrame[] {
    return this.pushText(this.#decoder.decode(chunk, { stream: true }));
  }

  /**
   * Push a text chunk directly. Used by the parser tests so they can
   * exercise byte-boundary edge cases without manufacturing a
   * `TextDecoder` round-trip.
   */
  pushText(text: string): readonly SseFrame[] {
    this.#buffer += text;
    const frames: SseFrame[] = [];
    let idx: number;
    // SSE accepts \n, \r\n, or \r as the line terminator. Normalize by
    // splitting on any of them — but only split on a complete line
    // (we leave trailing partial bytes in the buffer).
    while ((idx = findLineBreak(this.#buffer)) !== -1) {
      const line = this.#buffer.slice(0, idx);
      // A trailing CR may be the first half of a CRLF split across chunks.
      // A blank trailing CR can dispatch immediately without splitting data.
      if (this.#buffer[idx] === "\r" && idx === this.#buffer.length - 1 && line.length > 0) break;
      const skip = this.#buffer[idx] === "\r" && this.#buffer[idx + 1] === "\n" ? 2 : 1;
      this.#buffer = this.#buffer.slice(idx + skip);
      const frame = this.#consumeLine(line);
      if (frame) frames.push(frame);
    }
    return frames;
  }

  /**
   * Indicate end-of-stream. Returns any final frame that the dispatcher
   * was holding when the stream closed mid-frame (no trailing blank
   * line). The dispatcher pattern is: blank line dispatches; if the
   * remote closes after a partial frame, that frame is dropped per the
   * spec.
   */
  flush(): void {
    // No-op for now — partial frames are discarded per spec. Kept as
    // an extension point in case callers want a "soft flush" later.
  }

  #consumeLine(line: string): SseFrame | null {
    if (line.length === 0) {
      // Blank line → dispatch the current frame.
      return this.#dispatch();
    }
    if (line.startsWith(":")) {
      // Comment — ignored.
      return null;
    }
    const colonIdx = line.indexOf(":");
    let field: string;
    let value: string;
    if (colonIdx === -1) {
      field = line;
      value = "";
    } else {
      field = line.slice(0, colonIdx);
      value = line.slice(colonIdx + 1);
      if (value.startsWith(" ")) value = value.slice(1);
    }
    switch (field) {
      case "event":
        this.#eventType = value;
        break;
      case "data":
        this.#dataLines.push(value);
        break;
      case "id":
        // Per spec: empty id sets last id to empty string; ids
        // containing U+0000 are ignored. We adopt the simple
        // policy and just store the verbatim value.
        if (!value.includes("\0")) {
          this.#lastEventId = value;
        }
        break;
      case "retry": {
        const parsed = Number.parseInt(value, 10);
        if (Number.isFinite(parsed) && parsed >= 0) this.#retry = parsed;
        break;
      }
      default:
        // Unknown field — ignored per spec.
        break;
    }
    return null;
  }

  #dispatch(): SseFrame | null {
    if (this.#dataLines.length === 0 && this.#eventType === "") {
      // Nothing to dispatch — a blank line on its own resets nothing.
      return null;
    }
    const data = this.#dataLines.join("\n");
    const event = this.#eventType === "" ? "message" : this.#eventType;
    const frame: SseFrame = {
      event,
      data,
      ...(this.#lastEventId !== undefined ? { id: this.#lastEventId } : {}),
      ...(this.#retry !== undefined ? { retry: this.#retry } : {})
    };
    // Reset per-frame state. `lastEventId` and `retry` PERSIST across
    // frames per spec — they are connection-level, not frame-level.
    this.#eventType = "";
    this.#dataLines = [];
    this.#retry = undefined;
    return frame;
  }

  /**
   * Current "last event id" — the cursor the SSE spec instructs the
   * client to replay on automatic reconnection. Callers use this to
   * resume after a disconnect.
   */
  get lastEventId(): string | undefined {
    return this.#lastEventId;
  }
}

/**
 * Convert a web `ReadableStream<Uint8Array>` (the body of a fetch
 * Response) into an async iterable of SSE frames. The stream is
 * consumed exactly once; the caller is responsible for cancelling
 * the underlying reader on abort.
 *
 * The reader is raced against the abort signal so a long-blocked
 * server (no chunks for many seconds) still releases the loop
 * promptly when the consumer aborts.
 */
export async function* iterateSse(
  body: ReadableStream<Uint8Array>,
  signal?: AbortSignal
): AsyncIterable<SseFrame> {
  const reader = body.getReader();
  // Derived from the reader so the annotation follows the ambient stream lib
  // (@types/node vs bun-types disagree on the read-result shape).
  type ReadResult = Awaited<ReturnType<typeof reader.read>>;
  const parser = new SseParser();
  const ABORT_SENTINEL = Symbol("aborted");
  try {
    while (true) {
      if (signal?.aborted) return;
      const readPromise = reader.read();
      let result: ReadResult | typeof ABORT_SENTINEL;
      if (signal) {
        result = await Promise.race<ReadResult | typeof ABORT_SENTINEL>([
          readPromise,
          new Promise((resolve) => {
            const onAbort = (): void => resolve(ABORT_SENTINEL);
            if (signal.aborted) {
              resolve(ABORT_SENTINEL);
              return;
            }
            signal.addEventListener("abort", onAbort, { once: true });
          })
        ]);
      } else {
        result = await readPromise;
      }
      if (result === ABORT_SENTINEL) return;
      if (result.done) {
        parser.flush();
        return;
      }
      const value = result.value;
      if (!value) continue;
      const frames = parser.pushBytes(value);
      for (const frame of frames) {
        yield frame;
      }
    }
  } finally {
    try {
      await reader.cancel();
    } catch {
      // Cancel can throw if the stream is already closed — ignore.
    }
  }
}

function findLineBreak(buffer: string): number {
  // Returns the index of the first \n or \r in `buffer`, or -1.
  for (let i = 0; i < buffer.length; i++) {
    const code = buffer.charCodeAt(i);
    if (code === 0x0a || code === 0x0d) return i;
  }
  return -1;
}
