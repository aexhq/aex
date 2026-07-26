/**
 * The seam the wire-conformance harness (C4) attaches to.
 *
 * `HttpClient` reports every JSON response here — 2xx AND the error envelopes it
 * is about to throw on. Nothing observes it unless a harness installs an
 * observer, so the shipped cost is one null check per request and this file's
 * handful of bytes — deliberately small, because it ships in the published SDK
 * while the harness that uses it does not.
 *
 * It lives here rather than in `schemas/` because `http.ts` must not pull the
 * schema library in on the request path.
 */

/** A JSON response as it came off the wire, before any client-side shaping. */
export interface WireResponse {
  readonly method: string;
  /**
   * Scheme + host + port of the plane that served it — `https://api.aex.dev`.
   *
   * Carried because a PATH does not identify a route: the control plane serves
   * different bodies at two of the data plane's paths (`GET /api/whoami`,
   * `DELETE /api/workspaces/{id}`). Without this, a process driving both planes
   * validates a control-plane body against a data-plane schema and reports a
   * violation that is not one. `URL.origin` is a getter on a URL the client has
   * already constructed, and it is read inside the reporting thunk, so nothing
   * computes it when no harness is observing.
   */
  readonly origin: string;
  /** Path only — no origin, no query string. `/api/sessions/sess_1/messages`. */
  readonly path: string;
  readonly status: number;
  /**
   * The body EXACTLY as it was decoded, including on a non-2xx.
   *
   * Never the client's enrichment of it: `HttpClient` folds the `x-request-id`
   * header into the error body it throws, and a harness validating that derived
   * object would be checking our own client rather than the server's bytes.
   */
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
