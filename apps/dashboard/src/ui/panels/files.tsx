"use client";

import { useState, type FormEvent } from "react";
import type { DownloadGrant, RegisteredFile, RegisteredFilePage } from "@aexhq/sdk";

import { DEADLINE_MS, submit, useResource } from "../client";
import { Badge, Card, Empty, Notice, Resolved } from "../components";
import { instant, label } from "../status";

async function sha256(data: Uint8Array): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", data as BufferSource);
  return [...new Uint8Array(digest)].map((value) => value.toString(16).padStart(2, "0")).join("");
}

export function FilesPanel({ region }: { region: string }) {
  const files = useResource<RegisteredFilePage>("registry_files_list", {
    region, parameters: { limit: "100" }, deadlineMs: DEADLINE_MS.resources,
  });
  const [problem, setProblem] = useState<string | null>(null);
  const [grant, setGrant] = useState<DownloadGrant | null>(null);

  async function replace(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    const name = String(data.get("name") ?? "");
    const source = String(data.get("source") ?? "");
    const mediaType = String(data.get("mediaType") ?? "application/octet-stream");
    const url = String(data.get("url") ?? "");
    const text = String(data.get("text") ?? "");
    const content = source === "url"
      ? { type: "url" as const, url }
      : await (async () => {
          const bytes = new TextEncoder().encode(text);
          return { type: "inline" as const, encoding: "utf8" as const, data: text, sha256: await sha256(bytes) };
        })();
    const result = await submit<RegisteredFile>("registry_files_put", {
      region, parameters: { name }, body: { content, mediaType, mode: "0644" },
    });
    if (result.kind === "ready") {
      setProblem(null);
      form.reset();
      files.reload();
    } else setProblem("failure" in result ? result.failure.message : "The file was not registered.");
  }

  return <Card title="Files" description="Latest-only workspace files. Replace by URL or inline text; prior values are not addressable." flush>
    <div className="stack" style={{ padding: "var(--aex-space-4)" }}>
      {problem ? <Notice status="warning" title="File change failed" live><p>{problem}</p></Notice> : null}
      {grant ? <Notice status="good" title="Download ready" live><a className="button" href={grant.url}>Download</a></Notice> : null}
      <form className="stack" onSubmit={(event) => void replace(event)}>
        <div className="row"><label className="field"><span>Name</span><input name="name" required /></label><label className="field"><span>Media type</span><input name="mediaType" defaultValue="text/plain" required /></label><label className="field"><span>Source</span><select name="source"><option value="inline">Inline text</option><option value="url">HTTPS URL</option></select></label></div>
        <label className="field"><span>Inline text</span><textarea name="text" rows={5} /></label>
        <label className="field"><span>HTTPS URL</span><input name="url" type="url" /></label>
        <p><button className="button" data-variant="primary">Replace current file</button></p>
      </form>
    </div>
    <Resolved state={files.state} reload={files.reload}>{(page) => page.items.length === 0 ? <Empty title="No files." /> : <table><tbody>{page.items.map((file) => <tr key={file.name}><td className="mono">{file.name}</td><td><Badge label={label(file.state)} status={file.state === "ready" ? "good" : file.state === "failed" ? "serious" : "warning"} /></td><td>{instant(file.updatedAt)}</td><td><button className="button" disabled={file.state !== "ready"} onClick={() => void submit<DownloadGrant>("registry_files_download_create", { region, parameters: { name: file.name }, body: {} }).then((result) => { if (result.kind === "ready") setGrant(result.data); })}>Download</button> <button className="button" onClick={() => void submit("registry_files_delete", { region, parameters: { name: file.name } }).then(() => files.reload())}>Delete</button></td></tr>)}</tbody></table>}</Resolved>
  </Card>;
}
