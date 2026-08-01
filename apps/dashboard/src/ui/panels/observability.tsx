"use client";

import { useState } from "react";
import type { RouteId } from "@aexhq/sdk";

import { DEADLINE_MS, useResource } from "../client";
import { Badge, Card, CoverageNotice, Empty, Resolved } from "../components";
import { readCoverage } from "../panel";
import { bytes, instant, label } from "../status";
import type { ObservationPage, Page, TelemetryGap } from "../wire";

const WINDOWS = [
  { label: "Last hour", hours: 1 },
  { label: "Last 24 hours", hours: 24 },
  { label: "Last 7 days", hours: 24 * 7 },
] as const;

const SIGNALS = [
  { key: "events", routeId: "observations_events_query" satisfies RouteId, title: "Events" },
  { key: "traces", routeId: "observations_traces_query" satisfies RouteId, title: "Traces" },
] as const;

type SignalKey = (typeof SIGNALS)[number]["key"];

function windowFor(hours: number, now: number) {
  const stamp = (value: number) => new Date(value).toISOString().replace(/\.\d{3}Z$/, ".000Z");
  return { gte: stamp(now - hours * 3_600_000), lt: stamp(now) };
}

/**
 * Workspace observability.
 *
 * Two signals, because two are what this surface can answer honestly. Metric
 * aggregation is not offered: the contract has no metric-name discovery operation,
 * so the only metrics panel that could exist would be a free-text box, and an empty
 * result from a mistyped metric name is indistinguishable from an empty result from
 * a real one. Logs, spans and raw telemetry queries exist on the wire and are left
 * to the SDK and CLI rather than given a fourth near-identical table here.
 */
export function ObservabilityPanels({
  slug,
  region,
  billingHref,
}: {
  slug: string;
  region: string;
  billingHref?: string | undefined;
}) {
  const [signal, setSignal] = useState<SignalKey>("events");
  const [hours, setHours] = useState<number>(24);
  const [now, setNow] = useState(() => Date.now());
  const active = SIGNALS.find((candidate) => candidate.key === signal) ?? SIGNALS[0];
  const timeRange = windowFor(hours, now);

  const { state, reload } = useResource<ObservationPage>(active.routeId, {
    region,
    body: { signal: active.key, timeRange, limit: 100, order: "descending", consistency: "indexed" },
    deadlineMs: DEADLINE_MS.analytics,
  });

  return (
    <div className="stack">
      <Card
        title={active.title}
        description={`Workspace ${active.key} observations, newest first.`}
        actions={
          <>
            <label className="field">
              <span className="sr-only">Signal</span>
              <select value={signal} onChange={(event) => setSignal(event.target.value as SignalKey)}>
                {SIGNALS.map((candidate) => (
                  <option key={candidate.key} value={candidate.key}>{candidate.title}</option>
                ))}
              </select>
            </label>
            <label className="field">
              <span className="sr-only">Window</span>
              <select value={hours} onChange={(event) => setHours(Number(event.target.value))}>
                {WINDOWS.map((option) => (
                  <option key={option.hours} value={option.hours}>{option.label}</option>
                ))}
              </select>
            </label>
            <button
              type="button"
              className="button"
              onClick={() => {
                setNow(Date.now());
                reload();
              }}
            >
              Refresh
            </button>
          </>
        }
        flush
      >
        <Resolved state={state} reload={reload} billingHref={billingHref}>
          {(page) => {
            const verdict = readCoverage(page.coverage);
            return (
              <>
                <div style={{ padding: "var(--space-4)" }} className="stack">
                  {verdict.kind === "complete" ? (
                    <p className="small muted">
                      Complete through {instant(new Date(verdict.completeThrough).toISOString())}, with no
                      known holes in this window.
                    </p>
                  ) : (
                    <CoverageNotice verdict={verdict} />
                  )}
                </div>
                {page.items.length === 0 ? (
                  <Empty
                    title={
                      verdict.kind === "complete"
                        ? `No ${active.key} in this window.`
                        : `No ${active.key} survived in this window.`
                    }
                    hint={
                      verdict.kind === "complete"
                        ? "The window is whole, so this is an answer."
                        : "The window is not whole; this is not the same as nothing happening."
                    }
                  />
                ) : (
                  <div className="scroller">
                    <table>
                      <caption className="sr-only">Workspace {active.key}</caption>
                      <thead>
                        <tr>
                          <th scope="col">Observed</th>
                          <th scope="col">Sequence</th>
                          <th scope="col">Session</th>
                          <th scope="col">Trace</th>
                        </tr>
                      </thead>
                      <tbody>
                        {page.items.map((observation) => (
                          <tr key={observation.id}>
                            <td className="small muted">{instant(observation.observedAt)}</td>
                            <td className="numeric">{observation.sequence}</td>
                            <td className="mono small">
                              {observation.sessionId ? (
                                <a href={`/w/${slug}/sessions/${observation.sessionId}`}>
                                  {observation.sessionId}
                                </a>
                              ) : (
                                "—"
                              )}
                            </td>
                            <td className="mono small">
                              {/* A trace is only readable inside a session; the contract has no
                                  workspace-scoped trace read, so an unattributed trace id is shown
                                  but not offered as a link that cannot work. */}
                              {observation.traceId && observation.sessionId ? (
                                <a
                                  href={`/w/${slug}/sessions/${observation.sessionId}/traces/${observation.traceId}`}
                                >
                                  {observation.traceId.slice(0, 12)}…
                                </a>
                              ) : observation.traceId ? (
                                `${observation.traceId.slice(0, 12)}…`
                              ) : (
                                "—"
                              )}
                            </td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                )}
                {page.nextCursor ? (
                  <p className="placeholder small">
                    More observations exist in this window than one page holds. Narrow the window to
                    see the rest.
                  </p>
                ) : null}
              </>
            );
          }}
        </Resolved>
      </Card>

      <GapsPanel region={region} hours={hours} now={now} billingHref={billingHref} />
    </div>
  );
}

/**
 * What was lost, and whether it can come back.
 *
 * The coverage notice names the gaps that qualify one answer; this names every gap
 * recorded in the window with its cause, size and recoverability. They are
 * complementary, not two views of the same number.
 */
function GapsPanel({
  region,
  hours,
  now,
  billingHref,
}: {
  region: string;
  hours: number;
  now: number;
  billingHref?: string | undefined;
}) {
  const { state, reload } = useResource<Page<TelemetryGap>>("telemetry_gaps_query", {
    region,
    body: { timeRange: windowFor(hours, now), limit: 50 },
    deadlineMs: DEADLINE_MS.analytics,
  });

  return (
    <Card title="Recorded gaps" description="Holes the authority knows about. There is no repair route." flush>
      <Resolved state={state} reload={reload} billingHref={billingHref}>
        {(page) =>
          page.items.length === 0 ? (
            <Empty title="No gaps recorded in this window." />
          ) : (
            <div className="scroller">
              <table>
                <caption className="sr-only">Recorded telemetry gaps</caption>
                <thead>
                  <tr>
                    <th scope="col">Detected</th>
                    <th scope="col">Cause</th>
                    <th scope="col">Signals</th>
                    <th scope="col">Lost</th>
                    <th scope="col">Recoverable</th>
                  </tr>
                </thead>
                <tbody>
                  {page.items.map((gap) => (
                    <tr key={gap.id}>
                      <td className="small muted">{instant(gap.detectedAt)}</td>
                      <td className="small">{label(gap.reason)}</td>
                      <td className="small muted">{gap.signals.join(", ")}</td>
                      <td className="numeric">
                        {gap.observationCount ? `${gap.observationCount} obs` : "—"}
                        {gap.byteCount ? ` · ${bytes(gap.byteCount)}` : ""}
                      </td>
                      <td>
                        {gap.repairedAt ? (
                          <Badge status="good" label="Repaired" />
                        ) : gap.recoverable ? (
                          <Badge status="warning" label="Recoverable" />
                        ) : (
                          <Badge status="critical" label="Lost" />
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )
        }
      </Resolved>
    </Card>
  );
}
