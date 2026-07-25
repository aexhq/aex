/**
 * The seam the wire-conformance harness (C4) attaches to.
 *
 * `HttpClient` reports every successful JSON response here. Nothing observes it
 * unless a harness installs an observer, so the shipped cost is one null check
 * per request and this file's handful of bytes — deliberately small, because it
 * ships in the published SDK while the harness that uses it does not.
 *
 * It lives here rather than in `schemas/` because `http.ts` must not pull the
 * schema library in on the request path.
 */

/** A JSON response as it came off the wire, before any client-side shaping. */
export interface WireResponse {
  readonly method: string;
  /** Path only — no origin, no query string. `/api/sessions/sess_1/messages`. */
  readonly path: string;
  readonly status: number;
  readonly body: unknown;
}

export type WireResponseObserver = (response: WireResponse) => void;

let observer: WireResponseObserver | undefined;

/**
 * Install an observer and get a function that removes it.
 *
 * Deliberately single-slot: two harnesses observing at once would double-count
 * coverage, and there is no case for it.
 */
export function observeWireResponses(next: WireResponseObserver): () => void {
  observer = next;
  return () => {
    if (observer === next) {
      observer = undefined;
    }
  };
}

/** Report a response. A no-op — and nearly free — when nothing is observing. */
export function reportWireResponse(response: () => WireResponse): void {
  if (observer === undefined) {
    return;
  }
  observer(response());
}
