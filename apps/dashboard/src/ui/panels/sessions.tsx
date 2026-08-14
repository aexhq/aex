"use client";

import { useState, type FormEvent } from "react";
import type { Session, SessionListPage } from "@aexhq/sdk";

import { DEADLINE_MS, submit, useResource } from "../client";
import { Badge, Card, Empty, Notice, Resolved } from "../components";
import { instant, label, sessionStatus } from "../status";

const STATUSES = [
  "idle",
  "running",
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
  const [problem, setProblem] = useState<string | null>(null);
  const { state, reload } = useResource<SessionListPage>("sessions_list", {
    region,
    parameters: { limit: "50", ...(status ? { status } : {}) },
    deadlineMs: DEADLINE_MS.control,
  });

  async function create(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    const result = await submit<Session>("session_create", {
      region,
      body: {
        provider: String(data.get("provider") ?? "openai"),
        model: String(data.get("model") ?? ""),
        providerApiKey: String(data.get("providerApiKey") ?? ""),
        sandbox: { enabled: data.get("sandbox") === "on" },
      },
    });
    if (result.kind === "ready") globalThis.location.assign(`/w/${slug}/sessions/${result.data.id}`);
    else setProblem("failure" in result ? result.failure.message : "The session was not created.");
  }

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
      <div className="stack" style={{ padding: "var(--aex-space-4)" }}>
        {problem ? <Notice status="warning" title="Session creation failed" live><p>{problem}</p></Notice> : null}
        <form className="row" onSubmit={(event) => void create(event)}>
          <label className="field"><span>Provider</span><select name="provider"><option>openai</option><option>anthropic</option><option>deepseek</option><option>xai</option><option>meta</option><option>moonshotai</option><option>alibaba</option></select></label>
          <label className="field"><span>Model</span><input name="model" required /></label>
          <label className="field"><span>API key (write-only)</span><input name="providerApiKey" type="password" autoComplete="off" required /></label>
          <label className="row small"><input name="sandbox" type="checkbox" defaultChecked />Sandbox</label>
          <button className="button" data-variant="primary">Create</button>
        </form>
      </div>
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
