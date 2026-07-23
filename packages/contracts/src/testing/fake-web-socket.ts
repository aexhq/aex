/**
 * Canonical in-memory WebSocket test double for the `WebSocketLike` transport
 * contract — the single-sourced union of the nine copies that previously
 * drifted across sdk/contracts/cli tests (and the platform mirror).
 *
 * Tests drive it directly: `open()`/`message()`/`pong()`/`close()` emit to the
 * listeners the client attached; `sent` records outbound frames (keep-alive
 * pings); `sessionId` is derived from the subscribe URL for fake-platform
 * harnesses; `driven` is scratch bookkeeping for such harnesses.
 */
import type { WebSocketLike } from "../event-stream-client.js";

export class FakeWebSocket implements WebSocketLike {
  readonly url: string;
  /** Last path segment of the URL with `?ticket=&from=` query params stripped. */
  readonly sessionId: string;
  /** Outbound frames pushed through send() (e.g. keep-alive pings). */
  readonly sent: string[] = [];
  /** Set once close() runs. */
  closed = false;
  /** Scratch flag for driver harnesses (e.g. a fake platform marking the socket driven). */
  driven = false;
  readonly #listeners: Record<string, Array<(ev: { data?: unknown }) => void>> = {};

  constructor(url: string) {
    this.url = url;
    this.sessionId = (url.split("?")[0] ?? url).split("/").pop() ?? "";
  }

  addEventListener(type: "open" | "message" | "close" | "error", listener: (ev: { data?: unknown }) => void): void {
    (this.#listeners[type] ??= []).push(listener);
  }

  removeEventListener(): void {
    /* listeners live for the socket's lifetime in tests */
  }

  send(data: string): void {
    this.sent.push(data);
  }

  close(): void {
    this.closed = true;
    this.#emit("close", {});
  }

  /** Emit the "open" handshake event. */
  open(): void {
    this.#emit("open", {});
  }

  /** Deliver one event frame, JSON-encoded exactly as the wire would carry it. */
  message(event: unknown): void {
    this.#emit("message", { data: JSON.stringify(event) });
  }

  /** A keep-alive pong (or any non-event frame): proves liveness, carries no sequence. */
  pong(data = "aex:pong"): void {
    this.#emit("message", { data });
  }

  #emit(type: string, ev: { data?: unknown }): void {
    for (const cb of this.#listeners[type] ?? []) cb(ev);
  }
}
