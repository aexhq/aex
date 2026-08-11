"use client";

import { useState } from "react";

import { DEADLINE_MS, submit, useResource } from "../client";
import { Badge, Card, CoverageNotice, Empty, Notice, Resolved } from "../components";
import { readCoverage, type PanelState } from "../panel";
import { bytes, instant, label, sessionStatus } from "../status";
import type {
  Approval,
  LiveFileEntryPage,
  Message,
  ObservationPage,
  Page,
  Session,
} from "../wire";

interface Scope {
  readonly slug: string;
  readonly region: string;
  readonly sessionId: string;
  readonly billingHref?: string | undefined;
}

export function SessionHeader({ slug, region, sessionId, billingHref }: Scope) {
  const { state, reload } = useResource<Session>("session_get", {
    region,
    parameters: { sessionId },
    deadlineMs: DEADLINE_MS.control,
  });
  return (
    <Card title="Session" description={sessionId}>
      <Resolved state={state} reload={reload} billingHref={billingHref}>
        {(session) => (
          <div className="row">
            <Badge status={sessionStatus(session.status)} label={label(session.status)} />
            <span className="small">
              {session.resolvedConfig.provider}
              <span className="muted"> / </span>
              {session.resolvedConfig.model}
            </span>
            <span className="small muted">revision {session.revision}</span>
            <span className="small muted">created {instant(session.createdAt)}</span>
            <span className="small muted">updated {instant(session.updatedAt)}</span>
            <span className="spacer" />
            <a className="button" href={`/w/${slug}/sessions`}>All sessions</a>
          </div>
        )}
      </Resolved>
    </Card>
  );
}

export function MessagesPanel({ region, sessionId, billingHref }: Scope) {
  const { state, reload } = useResource<Page<Message>>("session_messages_list", {
    region,
    parameters: { sessionId, limit: "50" },
    deadlineMs: DEADLINE_MS.control,
  });
  return (
    <Card title="Messages" description="Complete sealed messages in immutable visibility order." flush>
      <Resolved state={state} reload={reload} billingHref={billingHref}>
        {(page) =>
          page.items.length === 0 ? (
            <Empty title="No messages yet." hint="Send text from the SDK or CLI to start the conversation." />
          ) : (
            <div className="scroller">
              <table>
                <caption className="sr-only">Messages in this session</caption>
                <thead>
                  <tr>
                    <th scope="col">Role</th>
                    <th scope="col">Message</th>
                    <th scope="col">Created</th>
                    <th scope="col">Content</th>
                  </tr>
                </thead>
                <tbody>
                  {page.items.map((message) => (
                    <tr key={message.id}>
                      <td><Badge label={label(message.role)} /></td>
                      <td className="mono small">{message.id}</td>
                      <td className="small muted">{instant(message.createdAt)}</td>
                      <td className="small">
                        {message.content.map((part) =>
                          part.type === "text" ? part.text : `${label(part.type)} ${part.id}`
                        ).join("\n")}
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

export function ApprovalsPanel({ region, sessionId, billingHref }: Scope) {
  const { state, reload } = useResource<Page<Approval>>("session_approvals_list", {
    region,
    parameters: { sessionId, limit: "50" },
    deadlineMs: DEADLINE_MS.control,
  });
  const [pending, setPending] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<PanelState<unknown> | null>(null);

  async function decide(approvalId: string, decision: "approve" | "deny") {
    setPending(approvalId);
    const result = await submit<Approval>("session_approval_respond", {
      region,
      parameters: { sessionId, approvalId },
      body: { decision },
    });
    setPending(null);
    setOutcome(result);
    if (result.kind === "ready") reload();
  }

  return (
    <Card
      title="Approvals"
      description="A bound call runs only if it is approved here or through a client."
    >
      <Resolved state={state} reload={reload} billingHref={billingHref}>
        {(page) =>
          page.items.length === 0 ? (
            <Empty title="Nothing is waiting on a decision." />
          ) : (
            <div className="stack">
              {outcome && outcome.kind !== "ready" ? (
                <Notice status="warning" title="That decision was not recorded" live>
                  <p className="small">
                    {"failure" in outcome ? outcome.failure.message : "The request did not complete."}
                  </p>
                </Notice>
              ) : null}
              {page.items.map((approval) => (
                <div key={approval.id} className="stack-tight">
                  <div className="row">
                    <Badge
                      status={approval.status === "pending" ? "warning" : undefined}
                      label={label(approval.status)}
                    />
                    <strong className="small">{approval.boundCall.toolName}</strong>
                    <span className="small muted">expires {instant(approval.expiresAt)}</span>
                  </div>
                  <p className="small muted mono">args {approval.boundCall.argumentsDigest.slice(0, 16)}…</p>
                  {approval.status === "pending" ? (
                    <div className="row">
                      <button
                        type="button"
                        className="button"
                        data-variant="primary"
                        disabled={pending === approval.id}
                        onClick={() => void decide(approval.id, "approve")}
                      >
                        Approve
                      </button>
                      <button
                        type="button"
                        className="button"
                        disabled={pending === approval.id}
                        onClick={() => void decide(approval.id, "deny")}
                      >
                        Deny
                      </button>
                    </div>
                  ) : (
                    <p className="small muted">
                      {approval.decision ? `${label(approval.decision)}d` : "Resolved"}{" "}
                      {instant(approval.resolvedAt)}
                    </p>
                  )}
                </div>
              ))}
            </div>
          )
        }
      </Resolved>
    </Card>
  );
}

export function LiveFilesPanel({ region, sessionId, billingHref }: Scope) {
  const { state, reload } = useResource<LiveFileEntryPage>("session_files_live_list", {
    region,
    parameters: { sessionId },
    body: { limit: 100, recursive: false },
    deadlineMs: DEADLINE_MS.resources,
  });

  return (
    <Card
      title="Live files"
      description="The ephemeral workspace in this session's exact retained generation. Reading automatically resumes a suspended generation."
      flush
    >
      <Resolved state={state} reload={reload} billingHref={billingHref}>
        {(page) =>
          page.items.length === 0 ? (
            <Empty
              title="No live files."
              hint={`Generation ${page.workspaceAccess.generationId}${page.workspaceAccess.resumed ? " resumed for this read" : " answered this read"}.`}
            />
          ) : (
            <>
              <p className="small muted" style={{ padding: "var(--aex-space-4)" }}>
                Generation {page.workspaceAccess.generationId}
                {page.workspaceAccess.resumed ? " resumed for this read." : " answered without resuming."}
              </p>
              <div className="scroller">
                <table>
                  <caption className="sr-only">Live files</caption>
                  <thead>
                    <tr>
                      <th scope="col">Path</th>
                      <th scope="col">Type</th>
                      <th scope="col">Size</th>
                      <th scope="col">Modified</th>
                      <th scope="col">Link target</th>
                    </tr>
                  </thead>
                  <tbody>
                    {page.items.map((entry) => (
                      <tr key={entry.path}>
                        <td className="mono small">{entry.path}</td>
                        <td className="small muted">{entry.type}</td>
                        <td className="numeric">{bytes(entry.sizeBytes)}</td>
                        <td className="small muted">{instant(entry.mtime)}</td>
                        <td className="mono small muted">{entry.target ?? "—"}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </>
          )
        }
      </Resolved>
    </Card>
  );
}

const WINDOW_HOURS = 24;

export function SessionEventsPanel({ slug, region, sessionId, billingHref }: Scope) {
  const [now] = useState(() => Date.now());
  const timeRange = {
    gte: new Date(now - WINDOW_HOURS * 3_600_000).toISOString().replace(/\.\d{3}Z$/, ".000Z"),
    lt: new Date(now).toISOString().replace(/\.\d{3}Z$/, ".000Z"),
  };
  const { state, reload } = useResource<ObservationPage>("session_observations_events_query", {
    region,
    parameters: { sessionId },
    body: { signal: "events", timeRange, limit: 100, order: "descending", consistency: "indexed" },
    deadlineMs: DEADLINE_MS.analytics,
  });

  return (
    <Card
      title="Events"
      description={`Session events observed in the last ${WINDOW_HOURS} hours.`}
      actions={<button type="button" className="button" onClick={reload}>Refresh</button>}
      flush
    >
      <Resolved state={state} reload={reload} billingHref={billingHref}>
        {(page) => {
          const verdict = readCoverage(page.coverage);
          return (
            <>
              {verdict.kind === "complete" ? null : (
                <div style={{ padding: "var(--aex-space-4)" }}>
                  <CoverageNotice verdict={verdict} />
                </div>
              )}
              {page.items.length === 0 ? (
                <Empty
                  title={
                    verdict.kind === "complete"
                      ? "No events in this window."
                      : "No events survived in this window."
                  }
                  hint={
                    verdict.kind === "complete"
                      ? "The window is whole, so this is an answer rather than a gap."
                      : "The window is not whole; absence here does not mean nothing happened."
                  }
                />
              ) : (
                <div className="scroller">
                  <table>
                    <caption className="sr-only">Session events</caption>
                    <thead>
                      <tr>
                        <th scope="col">Observed</th>
                        <th scope="col">Sequence</th>
                        <th scope="col">Trace</th>
                      </tr>
                    </thead>
                    <tbody>
                      {page.items.map((observation) => (
                        <tr key={observation.id}>
                          <td className="small muted">{instant(observation.observedAt)}</td>
                          <td className="numeric">{observation.sequence}</td>
                          <td className="mono small">
                            {observation.traceId ? (
                              <a href={`/w/${slug}/sessions/${sessionId}/traces/${observation.traceId}`}>
                                {observation.traceId.slice(0, 12)}…
                              </a>
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
            </>
          );
        }}
      </Resolved>
    </Card>
  );
}
