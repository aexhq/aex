"use client";

import { useState, type FormEvent } from "react";
import type { MessagePage, MessageSendResult, Session, TelemetryDownloadGrant } from "@aexhq/sdk";

import { DEADLINE_MS, panelUrl, submit, useResource } from "../client";
import { Badge, Card, Empty, Notice, Resolved } from "../components";
import { instant, label, sessionStatus } from "../status";

interface Scope {
  readonly slug: string;
  readonly region: string;
  readonly sessionId: string;
  readonly billingHref?: string | undefined;
}

export function SessionHeader({ slug, region, sessionId, billingHref }: Scope) {
  const session = useResource<Session>("session_get", { region, parameters: { sessionId }, deadlineMs: DEADLINE_MS.control });
  const [problem, setProblem] = useState<string | null>(null);

  async function command(routeId: "session_cancel" | "session_terminate" | "session_delete") {
    const result = await submit(routeId, { region, parameters: { sessionId }, body: {} });
    if (result.kind !== "ready") {
      setProblem("failure" in result ? result.failure.message : "The command was not accepted.");
      return;
    }
    if (routeId === "session_delete") globalThis.location.assign(`/w/${slug}/sessions`);
    else session.reload();
  }

  return <Card title="Session" description={sessionId}>
    {problem ? <Notice status="warning" title="Command failed" live><p>{problem}</p></Notice> : null}
    <Resolved state={session.state} reload={session.reload} billingHref={billingHref}>{(value) => <div className="row">
      <Badge status={sessionStatus(value.status)} label={label(value.status)} />
      <Badge label={`sandbox ${label(value.sandboxStatus)}`} />
      <span>{value.resolvedConfig.provider} / {value.resolvedConfig.model}</span>
      <span className="muted">updated {instant(value.updatedAt)}</span><span className="spacer" />
      <button className="button" onClick={() => void command("session_cancel")}>Cancel work</button>
      <button className="button" onClick={() => void command("session_terminate")}>Terminate sandbox</button>
      <button className="button" onClick={() => void command("session_delete")}>Delete session</button>
    </div>}</Resolved>
  </Card>;
}

export function MessagesPanel({ region, sessionId, billingHref }: Scope) {
  const messages = useResource<MessagePage>("session_messages_list", { region, parameters: { sessionId, limit: "100" }, deadlineMs: DEADLINE_MS.control });
  const [problem, setProblem] = useState<string | null>(null);
  async function send(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = event.currentTarget;
    const text = String(new FormData(form).get("text") ?? "");
    const result = await submit<MessageSendResult>("session_message_send", { region, parameters: { sessionId }, body: { text } });
    if (result.kind === "ready") { form.reset(); setProblem(null); messages.reload(); }
    else setProblem("failure" in result ? result.failure.message : "The message was not accepted.");
  }
  const stream = panelUrl("session_messages_stream", { sessionId }, region);
  return <Card title="Conversation" description="Committed messages are authoritative; the live feed carries bounded previews and reconcile/gap frames." flush>
    <div className="stack" style={{ padding: "var(--aex-space-4)" }}>
      {problem ? <Notice status="warning" title="Message failed" live><p>{problem}</p></Notice> : null}
      <form className="stack" onSubmit={(event) => void send(event)}><label className="field"><span>Message</span><textarea name="text" rows={5} required /></label><p><button className="button" data-variant="primary">Send</button> <a className="button" href={stream} target="_blank">Open live preview feed</a></p></form>
    </div>
    <Resolved state={messages.state} reload={messages.reload} billingHref={billingHref}>{(page) => page.items.length === 0 ? <Empty title="No messages." /> : <div className="stack" style={{ padding: "var(--aex-space-4)" }}>{page.items.map((message) => <article key={message.id}><p><Badge label={label(message.role)} /> <span className="muted">{instant(message.createdAt)}</span></p><pre>{message.content.map((part) => part.type === "text" ? part.text : part.type === "tool_call" ? `${part.name} ${JSON.stringify(part.arguments)}` : part.preview).join("\n")}</pre></article>)}</div>}</Resolved>
  </Card>;
}

export function TelemetryPanel({ region, sessionId }: Scope) {
  const [grant, setGrant] = useState<TelemetryDownloadGrant | null>(null);
  const live = panelUrl("session_telemetry_stream", { sessionId }, region);
  const replay = panelUrl("session_telemetry_replay", { sessionId, limit: "1000" }, region);
  return <Card title="Telemetry" description="The same normalized frames are transmitted live and retained for replay/download.">
    <p className="row"><a className="button" href={live} target="_blank">Live feed</a><a className="button" href={replay} target="_blank">Replay retained</a><button className="button" onClick={() => void submit<TelemetryDownloadGrant>("session_telemetry_download_create", { region, parameters: { sessionId }, body: {} }).then((result) => { if (result.kind === "ready") setGrant(result.data); })}>Create download</button>{grant ? <a className="button" data-variant="primary" href={grant.url}>Download</a> : null}</p>
  </Card>;
}
