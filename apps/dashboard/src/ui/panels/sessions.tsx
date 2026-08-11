"use client";

import { useState } from "react";

import { DEADLINE_MS, useResource } from "../client";
import { Badge, Card, Empty, Resolved } from "../components";
import { instant, label, sessionStatus } from "../status";
import type { Page, SessionListItem } from "../wire";

const STATUSES = [
  "idle",
  "running",
  "suspending",
  "suspended",
  "resuming",
  "terminating",
  "terminated",
  "deleting",
] as const;

export function SessionsPanel({
  slug,
  region,
  billingHref,
}: {
  slug: string;
  region: string;
  billingHref?: string;
}) {
  const [status, setStatus] = useState("");
  const { state, reload } = useResource<Page<SessionListItem>>("sessions_list", {
    region,
    parameters: { limit: "50", ...(status ? { status } : {}) },
    deadlineMs: DEADLINE_MS.control,
  });

  return (
    <Card
      title="Sessions"
      description="Every session in this workspace, newest activity first."
      actions={
        <label className="field">
          <span className="sr-only">Filter by status</span>
          <select value={status} onChange={(event) => setStatus(event.target.value)}>
            <option value="">All statuses</option>
            {STATUSES.map((value) => (
              <option key={value} value={value}>{label(value)}</option>
            ))}
          </select>
        </label>
      }
      flush
    >
      <Resolved state={state} reload={reload} {...(billingHref ? { billingHref } : {})}>
        {(page) =>
          page.items.length === 0 ? (
            <Empty
              title={status ? `No sessions with status ${label(status)}.` : "No sessions yet."}
              hint="Sessions are created from the SDK or the CLI; this dashboard observes them."
            />
          ) : (
            <>
              <div className="scroller">
                <table>
                  <caption className="sr-only">Sessions in this workspace</caption>
                  <thead>
                    <tr>
                      <th scope="col">Status</th>
                      <th scope="col">Session</th>
                      <th scope="col">Model</th>
                      <th scope="col">Created</th>
                      <th scope="col">Updated</th>
                    </tr>
                  </thead>
                  <tbody>
                    {page.items.map((session) => (
                      <tr key={session.id}>
                        <td><Badge {...{ status: sessionStatus(session.status) }} label={label(session.status)} /></td>
                        <td>
                          <a className="mono" href={`/w/${slug}/sessions/${session.id}`}>{session.id}</a>
                        </td>
                        <td className="small">
                          {session.provider}
                          <span className="muted"> / </span>
                          {session.model}
                        </td>
                        <td className="small muted">{instant(session.createdAt)}</td>
                        <td className="small muted">{instant(session.updatedAt)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              {page.nextCursor ? (
                <p className="placeholder small">
                  Showing the first {page.items.length}. More pages exist; open a session to work
                  with it, or narrow the status filter.
                </p>
              ) : null}
            </>
          )
        }
      </Resolved>
    </Card>
  );
}
