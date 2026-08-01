"use client";

import { useState } from "react";

import { DEADLINE_MS, useResource } from "../client";
import { Card, Empty, Notice, Resolved } from "../components";
import { centsToUsd, ratedThrough } from "../panel";
import { bytes, instant, label } from "../status";
import type { UsageAggregate, UsagePage } from "../wire";

const WINDOWS = [
  { label: "Last 24 hours", hours: 24 },
  { label: "Last 7 days", hours: 24 * 7 },
  { label: "Last 30 days", hours: 24 * 30 },
] as const;

/** The one measured quantity each category actually reports, in its own unit. */
function quantity(row: UsageAggregate): string {
  switch (row.category) {
    case "storage": return `${row.byteMinutes} byte-minutes`;
    case "compute": return `${row.millicpuMilliseconds} millicpu-ms`;
    case "memory": return `${row.byteMilliseconds} byte-ms`;
    case "data_transfer": return `${bytes(row.egressBytes)} egress`;
  }
}

/**
 * Rated usage.
 *
 * There is no chart. The four categories are measured in four incompatible units,
 * and the only figure they share is the rated amount, which is already exact — a
 * bar chart over four exact numbers adds a scale to be misread and nothing else.
 *
 * The total is clamped to the pipeline frontier: anything after `serviceThrough`
 * has been metered but not priced, so showing it as a total would state a cost the
 * authority has not computed. The clamp is stated, not silent.
 */
export function UsagePanel({
  workspaceId,
  region,
  billingHref,
}: {
  workspaceId: string;
  region: string;
  billingHref?: string | undefined;
}) {
  const [hours, setHours] = useState<number>(24 * 7);
  const [now, setNow] = useState(() => Date.now());
  const stamp = (value: number) => new Date(value).toISOString().replace(/\.\d{3}Z$/, ".000Z");
  const timeRange = { gte: stamp(now - hours * 3_600_000), lt: stamp(now) };

  const { state, reload } = useResource<UsagePage>("usage_query", {
    region,
    parameters: { workspaceId },
    body: { timeRange, groupBy: ["category"], limit: 200 },
    deadlineMs: DEADLINE_MS.analytics,
  });

  return (
    <Card
      title="Rated usage"
      description="What this workspace consumed, priced by the usage authority."
      actions={
        <>
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
          const frontier = ratedThrough(page.frontiers);
          const priced = frontier === null
            ? []
            : page.items.filter((row) => row.attribution.serviceTime.lt <= frontier);
          const withheld = page.items.length - priced.length;
          const total = priced.reduce((sum, row) => sum + Number(row.attribution.ratedCents), 0);

          return (
            <>
              <div style={{ padding: "var(--aex-space-4)" }} className="stack">
                {frontier === null ? (
                  <Notice status="serious" title="No pricing frontier was reported" live>
                    <p className="small">
                      Without a frontier there is no instant these numbers are complete through, so
                      no total is shown. The rows below are raw aggregates only.
                    </p>
                  </Notice>
                ) : (
                  <>
                    <div className="stat">
                      <span className="stat-label">Rated in this window</span>
                      <span className="stat-value">{centsToUsd(String(total))}</span>
                      <span className="stat-note">
                        Complete through {instant(frontier)}
                        {withheld > 0
                          ? ` · ${withheld} aggregate${withheld === 1 ? "" : "s"} measured after the frontier are excluded from this total`
                          : ""}
                      </span>
                    </div>
                    {withheld > 0 ? (
                      <Notice status="warning" title="Part of this window is not priced yet" live>
                        <p className="small">
                          {withheld} aggregate{withheld === 1 ? "" : "s"} fall after the pipeline
                          frontier. They are listed below and marked, and they are not in the total,
                          because their price has not been computed.
                        </p>
                      </Notice>
                    ) : null}
                  </>
                )}
              </div>

              {page.items.length === 0 ? (
                <Empty
                  title="No rated usage in this window."
                  hint="Usage is rated after service time; a very recent window can legitimately be empty."
                />
              ) : (
                <div className="scroller">
                  <table>
                    <caption className="sr-only">Rated usage aggregates</caption>
                    <thead>
                      <tr>
                        <th scope="col">Category</th>
                        <th scope="col">Quantity</th>
                        <th scope="col">Service time</th>
                        <th scope="col">Region</th>
                        <th scope="col">Rated</th>
                      </tr>
                    </thead>
                    <tbody>
                      {page.items.map((row, index) => {
                        const pending = frontier !== null && row.attribution.serviceTime.lt > frontier;
                        return (
                          <tr key={`${row.category}-${row.attribution.serviceTime.gte}-${index}`}>
                            <td className="small">{label(row.category)}</td>
                            <td className="small muted">{quantity(row)}</td>
                            <td className="small muted">
                              {instant(row.attribution.serviceTime.gte)}
                              {pending ? <span className="muted"> · not priced yet</span> : null}
                            </td>
                            <td className="small muted">{row.attribution.region}</td>
                            <td className="numeric">
                              {pending ? "—" : centsToUsd(row.attribution.ratedCents)}
                            </td>
                          </tr>
                        );
                      })}
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
