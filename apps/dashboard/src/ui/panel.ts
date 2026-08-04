/**
 * The panel state model.
 *
 * Every state below is a real wire outcome, not a UI mood. The rule the whole
 * dashboard is built on: a response that is not complete data is never rendered as
 * complete data, and a typed product state is never rendered as an error.
 *
 *   paused      402 `account_paused` — the account is a real product state
 *   throttled   429 `rate_limited` / `limit_exceeded` / `slow_down`
 *   unavailable 502/503/504 — `observability_unavailable`, `upstream_error`
 *   denied      403 `forbidden` / `insufficient_scope`
 *   expired     401 — the browser session is gone; re-authenticate
 *   missing     404/410 — the resource is gone, which is an answer, not a fault
 *   timeout     the panel's own deadline elapsed; nothing is known
 *   failed      anything else, always carrying the request id
 *
 * and on the success side:
 *
 *   empty       200 with no rows AND complete coverage — "there is nothing"
 *   degraded    200 the authority itself says is partial — "this is not all of it"
 *   ready       200 the authority says is complete
 */

export interface WireFailure {
  readonly code: string;
  readonly message: string;
  readonly retryable: boolean;
  readonly requestId?: string;
  readonly operationId?: string;
  readonly details?: unknown;
}

export type PanelFailure =
  | { readonly kind: "expired" }
  | { readonly kind: "denied"; readonly failure: WireFailure }
  | { readonly kind: "paused"; readonly failure: WireFailure }
  | { readonly kind: "throttled"; readonly failure: WireFailure; readonly retryAfterMs: number | null }
  | { readonly kind: "unavailable"; readonly failure: WireFailure; readonly retryAfterMs: number | null }
  | { readonly kind: "missing"; readonly failure: WireFailure }
  | { readonly kind: "timeout"; readonly deadlineMs: number }
  | { readonly kind: "failed"; readonly failure: WireFailure };

export type PanelState<T> =
  | { readonly kind: "loading" }
  | { readonly kind: "ready"; readonly data: T }
  | PanelFailure;

const UNTYPED: WireFailure = {
  code: "invalid_error_envelope",
  message: "the upstream returned a failure this client cannot read",
  retryable: false,
};

export function readFailure(body: unknown): WireFailure {
  const envelope = (body as { error?: Record<string, unknown> } | null)?.error;
  if (!envelope || typeof envelope["code"] !== "string" || typeof envelope["message"] !== "string") {
    return UNTYPED;
  }
  return {
    code: envelope["code"],
    message: envelope["message"],
    retryable: envelope["retryable"] === true,
    ...(typeof envelope["requestId"] === "string" ? { requestId: envelope["requestId"] } : {}),
    ...(typeof envelope["operationId"] === "string" ? { operationId: envelope["operationId"] } : {}),
    ...(envelope["details"] === undefined ? {} : { details: envelope["details"] }),
  };
}

export function classifyFailure(status: number, body: unknown, retryAfterMs: number | null): PanelFailure {
  const failure = readFailure(body);
  if (status === 401) return { kind: "expired" };
  if (status === 402 || failure.code === "account_paused") return { kind: "paused", failure };
  if (status === 403) return { kind: "denied", failure };
  if (status === 404 || status === 410) return { kind: "missing", failure };
  if (status === 429) return { kind: "throttled", failure, retryAfterMs };
  if (status >= 500) return { kind: "unavailable", failure, retryAfterMs };
  return { kind: "failed", failure };
}

export function parseRetryAfter(header: string | null): number | null {
  if (!header) return null;
  const seconds = Number(header);
  return Number.isFinite(seconds) && seconds >= 0 ? Math.ceil(seconds * 1000) : null;
}

/** The four states the *paused* account can put a panel in, from the route table. */
export function pausedExplanation(pauseExempt: boolean): string {
  return pauseExempt
    ? "This reading stays available while the account is paused."
    : "This operation does not answer while the account is paused.";
}

/* -------------------------------------------------------------- coverage --- */

export interface MissingInterval {
  readonly gapId: string;
  readonly range: { readonly gte: string; readonly lt: string };
}

export interface ObservationCoverage {
  readonly accepted: string;
  readonly indexed: string;
  readonly snapshot: string;
  readonly earliestReplay: string;
  readonly caughtUp: boolean;
  readonly complete: boolean;
  readonly missingIntervals: readonly MissingInterval[];
  readonly unboundedGaps: readonly string[];
}

export type CoverageVerdict =
  | { readonly kind: "complete"; readonly completeThrough: number }
  | { readonly kind: "behind"; readonly completeThrough: number; readonly lagMs: number }
  | {
      readonly kind: "incomplete";
      readonly completeThrough: number;
      readonly lagMs: number;
      readonly holes: readonly MissingInterval[];
      readonly unboundedGaps: readonly string[];
    };

/**
 * Read the coverage the authority attached to its own answer.
 *
 * `complete` and `caughtUp` are separate facts and are reported separately: a
 * window can be whole but still trail admission, and a caught-up window can still
 * have holes. Neither is ever rounded up into "here is your data".
 */
export function readCoverage(coverage: ObservationCoverage): CoverageVerdict {
  const accepted = Number(coverage.accepted);
  const indexed = Number(coverage.indexed);
  const lagMs = Number.isFinite(accepted) && Number.isFinite(indexed) ? Math.max(0, accepted - indexed) : 0;
  const holes = coverage.missingIntervals;
  if (!coverage.complete || holes.length > 0 || coverage.unboundedGaps.length > 0) {
    return {
      kind: "incomplete",
      completeThrough: indexed,
      lagMs,
      holes,
      unboundedGaps: coverage.unboundedGaps,
    };
  }
  if (!coverage.caughtUp) return { kind: "behind", completeThrough: indexed, lagMs };
  return { kind: "complete", completeThrough: indexed };
}

/* ----------------------------------------------------------------- usage --- */

export interface UsageFrontier {
  readonly category: string;
  readonly region: string;
  readonly serviceThrough?: string;
}

/**
 * The shared observation instant for every usage category in the answer.
 * Returning `null` means at least one category has not admitted a fact yet, so
 * the dashboard cannot claim a complete cross-category coverage instant.
 */
export function usageThrough(frontiers: readonly UsageFrontier[]): string | null {
  if (frontiers.length === 0) return null;
  let earliest: string | null = null;
  for (const frontier of frontiers) {
    if (frontier.serviceThrough === undefined) return null;
    if (earliest === null || frontier.serviceThrough < earliest) earliest = frontier.serviceThrough;
  }
  return earliest;
}

export function centsToUsd(cents: string): string {
  const value = Number(cents);
  if (!Number.isFinite(value)) return "—";
  return (value / 100).toLocaleString("en-US", { style: "currency", currency: "USD" });
}

export function formatDuration(milliseconds: number): string {
  if (milliseconds < 1_000) return `${milliseconds} ms`;
  if (milliseconds < 60_000) return `${(milliseconds / 1_000).toFixed(1)} s`;
  if (milliseconds < 3_600_000) return `${Math.round(milliseconds / 60_000)} min`;
  return `${(milliseconds / 3_600_000).toFixed(1)} h`;
}
