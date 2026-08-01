import type { ReactNode } from "react";

import {
  formatDuration,
  type CoverageVerdict,
  type PanelState,
  type WireFailure,
} from "./panel";

export type Status = "good" | "warning" | "serious" | "critical";

/** Four shapes, four roles. Colour never carries the meaning on its own. */
const GLYPH: Readonly<Record<Status, string>> = {
  good: "●",
  warning: "▲",
  serious: "◆",
  critical: "✕",
};

export function Badge({ status, label }: { status?: Status | undefined; label: string }) {
  if (!status) return <span className="badge">{label}</span>;
  return (
    <span className="badge" data-status={status}>
      <span className="badge-glyph" aria-hidden="true">{GLYPH[status]}</span>
      {label}
    </span>
  );
}

/**
 * The one branch between "there is an answer" and "there is not one yet".
 *
 * A panel never renders rows and a failure at the same time, and never renders an
 * empty table for a state that is not empty — a timeout, a throttle and a paused
 * account all produce no rows, and none of them means "you have nothing".
 */
export function Resolved<T>({
  state,
  reload,
  billingHref,
  children,
}: {
  state: PanelState<T>;
  reload?: (() => void) | undefined;
  billingHref?: string | undefined;
  children: (data: T) => ReactNode;
}) {
  if (state.kind === "ready") return <>{children(state.data)}</>;
  return (
    <PanelFallback state={state} onRetry={reload} billingHref={billingHref} />
  );
}

export function Notice({
  status,
  title,
  children,
  live,
}: {
  status: Status;
  title: string;
  children?: ReactNode;
  live?: boolean | undefined;
}) {
  return (
    <div className="notice" data-status={status} {...(live ? { role: "status" } : {})}>
      <span className="notice-glyph" aria-hidden="true">{GLYPH[status]}</span>
      <div className="stack-tight">
        <p className="notice-title">{title}</p>
        {children}
      </div>
    </div>
  );
}

export function Card({
  title,
  description,
  actions,
  children,
  flush,
}: {
  title: string;
  description?: string | undefined;
  actions?: ReactNode;
  children: ReactNode;
  flush?: boolean | undefined;
}) {
  return (
    <section className="card" aria-label={title}>
      <div className="card-head">
        <div className="stack-tight">
          <h2>{title}</h2>
          {description ? <p className="small muted">{description}</p> : null}
        </div>
        <span className="spacer" />
        {actions}
      </div>
      <div className={flush ? "card-body flush" : "card-body"}>{children}</div>
    </section>
  );
}

export function Loading({ rows = 3 }: { rows?: number | undefined }) {
  return (
    <div className="placeholder" aria-busy="true" aria-live="polite">
      <span className="sr-only">Loading</span>
      {Array.from({ length: rows }, (_, index) => (
        <span key={index} className="skeleton" style={{ width: `${90 - index * 18}%` }} />
      ))}
    </div>
  );
}

export function Empty({ title, hint }: { title: string; hint?: string | undefined }) {
  return (
    <div className="placeholder">
      <p>{title}</p>
      {hint ? <p className="small">{hint}</p> : null}
    </div>
  );
}

function Diagnostics({ failure }: { failure: WireFailure }) {
  return (
    <p className="small muted">
      <code className="mono">{failure.code}</code>
      {failure.requestId ? (
        <>
          {" · request "}
          <code className="mono">{failure.requestId}</code>
        </>
      ) : null}
    </p>
  );
}

function Retry({ onRetry, after }: { onRetry?: (() => void) | undefined; after: number | null }) {
  if (!onRetry) return null;
  return (
    <button type="button" className="button" onClick={onRetry}>
      Retry{after === null ? "" : ` (suggested after ${formatDuration(after)})`}
    </button>
  );
}

/**
 * The one place a non-ready panel state becomes pixels.
 *
 * Nothing here invents data and nothing here calls a typed product state an error.
 * A paused account, a throttle and an unavailable observation store each get their
 * own wording and their own remedy, because each has a different remedy.
 */
export function PanelFallback({
  state,
  onRetry,
  billingHref,
}: {
  state: Exclude<PanelState<unknown>, { kind: "ready"; data: unknown }>;
  onRetry?: (() => void) | undefined;
  billingHref?: string | undefined;
}) {
  switch (state.kind) {
    case "loading":
      return <Loading />;
    case "expired":
      return (
        <Notice status="warning" title="Your session ended" live>
          <p className="small">
            Sign in again to continue. Nothing was lost — the dashboard holds no state of its own.
          </p>
          <p>
            <a className="button" href="/signin">Sign in</a>
          </p>
        </Notice>
      );
    case "paused":
      return (
        <Notice status="serious" title="This account is paused" live>
          <p className="small">
            Paused accounts keep their data and their billing readings; execution and workspace
            reads are declined until the balance is restored.
          </p>
          {billingHref ? (
            <p>
              <a className="button" data-variant="primary" href={billingHref}>Review billing</a>
            </p>
          ) : null}
          <Diagnostics failure={state.failure} />
        </Notice>
      );
    case "denied":
      return (
        <Notice status="warning" title="You do not have access to this" live>
          <p className="small">{state.failure.message}</p>
          <Diagnostics failure={state.failure} />
        </Notice>
      );
    case "missing":
      return (
        <Notice status="warning" title="This no longer exists" live>
          <p className="small">{state.failure.message}</p>
          <Diagnostics failure={state.failure} />
        </Notice>
      );
    case "throttled":
      return (
        <Notice status="warning" title="Rate limited" live>
          <p className="small">{state.failure.message}</p>
          <p><Retry onRetry={onRetry} after={state.retryAfterMs} /></p>
          <Diagnostics failure={state.failure} />
        </Notice>
      );
    case "unavailable":
      return (
        <Notice status="serious" title="This reading is temporarily unavailable" live>
          <p className="small">
            {state.failure.message} Everything else on this page is unaffected.
          </p>
          <p><Retry onRetry={onRetry} after={state.retryAfterMs} /></p>
          <Diagnostics failure={state.failure} />
        </Notice>
      );
    case "timeout":
      return (
        <Notice status="serious" title="No answer within the panel deadline" live>
          <p className="small">
            The request was cancelled after {formatDuration(state.deadlineMs)}. Nothing is known
            about this reading — it is not empty, and it is not zero.
          </p>
          <p><Retry onRetry={onRetry} after={null} /></p>
        </Notice>
      );
    case "failed":
      return (
        <Notice status="critical" title="This request failed" live>
          <p className="small">{state.failure.message}</p>
          <p><Retry onRetry={onRetry} after={null} /></p>
          <Diagnostics failure={state.failure} />
        </Notice>
      );
  }
}

/**
 * What the observation authority said about its own answer, rendered verbatim.
 *
 * `complete` renders nothing: silence is the honest signal for whole data. The
 * other two verdicts are always shown above the rows they qualify, never below and
 * never behind a hover.
 */
export function CoverageNotice({ verdict }: { verdict: CoverageVerdict }) {
  if (verdict.kind === "complete") return null;
  if (verdict.kind === "behind") {
    return (
      <Notice status="warning" title="Indexing is behind admission" live>
        <p className="small">
          Complete through {new Date(verdict.completeThrough).toISOString()} — about{" "}
          {formatDuration(verdict.lagMs)} of accepted observations are not queryable yet. Recent
          activity may be missing from these rows.
        </p>
      </Notice>
    );
  }
  const holes = verdict.holes.length;
  const unbounded = verdict.unboundedGaps.length;
  return (
    <Notice status="serious" title="This window has known holes" live>
      <p className="small">
        The authority reports {holes === 0 ? "no bounded" : holes} missing interval
        {holes === 1 ? "" : "s"}
        {unbounded > 0 ? ` and ${unbounded} gap${unbounded === 1 ? "" : "s"} with no known bound` : ""}.
        These rows are what survived, not everything that happened.
      </p>
      {holes > 0 ? (
        <details>
          <summary>What is missing</summary>
          <ul className="small">
            {verdict.holes.map((hole) => (
              <li key={hole.gapId}>
                <code className="mono">{hole.gapId}</code> — {hole.range.gte} to {hole.range.lt}
              </li>
            ))}
          </ul>
        </details>
      ) : null}
      {unbounded > 0 ? (
        <details>
          <summary>Gaps with no known bound</summary>
          <ul className="small">
            {verdict.unboundedGaps.map((gapId) => (
              <li key={gapId}><code className="mono">{gapId}</code></li>
            ))}
          </ul>
        </details>
      ) : null}
    </Notice>
  );
}
