"use client";

import { useState } from "react";

import { DEADLINE_MS, useResource } from "../client";
import { Card, Empty, Notice, Resolved } from "../components";
import { usageThrough } from "../panel";
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
 * Measured usage.
 *
 * The regional usage authority reports four incompatible physical quantities.
 * Monetary rating belongs to central finance statements, so this panel neither
 * fabricates a price nor combines unlike units into a misleading total.
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
      title="Measured usage"
      description="Regional usage quantities. Monetary charges are recorded in central billing statements."
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
          const frontier = usageThrough(page.frontiers);

          return (
            <>
              <div style={{ padding: "var(--aex-space-4)" }} className="stack">
                {frontier === null ? (
                  <Notice status="warning" title="Usage coverage is still starting" live>
                    <p className="small">
                      At least one category has no service frontier yet. The measured rows below are
                      valid, but this view cannot claim a complete cross-category coverage instant.
                    </p>
                  </Notice>
                ) : (
                  <div className="stat">
                    <span className="stat-label">Shared service frontier</span>
                    <span className="stat-value">{instant(frontier)}</span>
                    <span className="stat-note">Earliest observed service time across the returned categories.</span>
                  </div>
                )}
              </div>

              {page.items.length === 0 ? (
                <Empty
                  title="No measured usage in this window."
                  hint="A new workspace or very recent window can legitimately be empty."
                />
              ) : (
                <div className="scroller">
                  <table>
                    <caption className="sr-only">Measured usage aggregates</caption>
                    <thead>
                      <tr>
                        <th scope="col">Category</th>
                        <th scope="col">Quantity</th>
                        <th scope="col">Service time</th>
                        <th scope="col">Region</th>
                      </tr>
                    </thead>
                    <tbody>
                      {page.items.map((row, index) => {
                        return (
                          <tr key={`${row.category}-${row.attribution.serviceTime.gte}-${index}`}>
                            <td className="small">{label(row.category)}</td>
                            <td className="small muted">{quantity(row)}</td>
                            <td className="small muted">
                              {instant(row.attribution.serviceTime.gte)}
                            </td>
                            <td className="small muted">{row.attribution.region}</td>
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
