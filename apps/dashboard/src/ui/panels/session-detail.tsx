"use client";

import { DEADLINE_MS, useResource } from "../client";
import { Badge, Card, Empty, Resolved } from "../components";
import { bytes, instant, label, sessionStatus } from "../status";
import type {
  LiveFileEntryPage,
  Message,
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
