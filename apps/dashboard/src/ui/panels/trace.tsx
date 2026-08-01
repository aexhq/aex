"use client";

import { DEADLINE_MS, useResource } from "../client";
import { Card, CoverageNotice, Empty, Resolved } from "../components";
import { readCoverage } from "../panel";
import { instant } from "../status";
import type { Observation, TraceDetail } from "../wire";

/**
 * One assembled trace.
 *
 * The waterfall is drawn from the span observations the authority actually
 * returned, positioned against the summary's own start and end. When coverage says
 * the assembly is incomplete the bar chart is still drawn — from real spans — but
 * the notice above it says the shape is partial, because a trace missing spans looks
 * exactly like a fast trace otherwise.
 */
export function TracePanel({
  region,
  sessionId,
  traceId,
  billingHref,
}: {
  region: string;
  sessionId: string;
  traceId: string;
  billingHref?: string | undefined;
}) {
  const { state, reload } = useResource<TraceDetail>("session_observations_trace_get", {
    region,
    parameters: { sessionId, traceId },
    deadlineMs: DEADLINE_MS.analytics,
  });

  return (
    <Card title="Trace" description={traceId} flush>
      <Resolved state={state} reload={reload} billingHref={billingHref}>
        {(trace) => {
          const verdict = readCoverage(trace.coverage);
          const started = Date.parse(trace.summary.startedAt);
          const ended = Date.parse(trace.summary.endedAt);
          const span = Math.max(1, ended - started);
          return (
            <>
              <div style={{ padding: "var(--space-4)" }} className="stack">
                {verdict.kind === "complete" ? null : <CoverageNotice verdict={verdict} />}
                <dl className="row small">
                  <div><dt className="muted">Root</dt><dd>{trace.summary.rootName ?? "—"}</dd></div>
                  <div><dt className="muted">Spans returned</dt><dd>{trace.spans.length} of {trace.summary.spanCount}</dd></div>
                  <div><dt className="muted">Started</dt><dd>{instant(trace.summary.startedAt)}</dd></div>
                  <div><dt className="muted">Ended</dt><dd>{instant(trace.summary.endedAt)}</dd></div>
                </dl>
              </div>
              {trace.spans.length === 0 ? (
                <Empty title="No spans were returned for this trace." />
              ) : (
                <div className="scroller">
                  <table>
                    <caption className="sr-only">Spans of this trace</caption>
                    <thead>
                      <tr>
                        <th scope="col">Span</th>
                        <th scope="col">Observed</th>
                        <th scope="col">Position in trace</th>
                      </tr>
                    </thead>
                    <tbody>
                      {trace.spans.map((observation) => (
                        <tr key={observation.id}>
                          <td className="mono small">{observation.spanId ?? observation.id}</td>
                          <td className="small muted">{instant(observation.observedAt)}</td>
                          <td><Offset observation={observation} started={started} span={span} /></td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </>
          );
        }}
      </Resolved>
    </Card>
  );
}

/**
 * A span's observed instant as a position inside the trace window. It is a
 * position, not a duration: the observation envelope carries one instant, so
 * drawing a width would be inventing an end time the API did not send.
 */
function Offset({
  observation,
  started,
  span,
}: {
  observation: Observation;
  started: number;
  span: number;
}) {
  const at = Date.parse(observation.observedAt);
  if (!Number.isFinite(at) || !Number.isFinite(started)) return <span className="small muted">—</span>;
  const fraction = Math.min(1, Math.max(0, (at - started) / span));
  return (
    <span className="offset" role="img" aria-label={`${Math.round(fraction * 100)} percent through the trace`}>
      <span className="offset-mark" style={{ insetInlineStart: `${fraction * 100}%` }} />
    </span>
  );
}
