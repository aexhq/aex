"use client";

import { useState, type FormEvent } from "react";

import { DEADLINE_MS, submit, useResource } from "../client";
import { Badge, Card, Empty, Notice, Resolved } from "../components";
import { instant } from "../status";
import type { ApiKey, NewApiKey, Page } from "../wire";

/**
 * Scopes offered when minting a key.
 *
 * These are the read and write scopes the dashboard's own operations require, taken
 * from the route registry. The contract's scope registry is the full list; offering
 * a longer menu here than the surfaces below can demonstrate would be inventing
 * capability, so the field also accepts a typed scope for anything else.
 */
const COMMON_SCOPES = [
  "sessions:read",
  "sessions:write",
  "telemetry:read",
  "provider_credentials:read",
  "provider_credentials:write",
  "resources:read",
  "resources:write",
  "billing:read",
] as const;

export function ApiKeysPanel({
  workspaceId,
  billingHref,
}: {
  workspaceId: string;
  billingHref?: string | undefined;
}) {
  const { state, reload } = useResource<Page<ApiKey>>("api_keys_list", {
    parameters: { workspaceId, limit: "100" },
    deadlineMs: DEADLINE_MS.control,
  });
  const [minted, setMinted] = useState<NewApiKey | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    const name = String(data.get("name") ?? "").trim();
    const scopes = data.getAll("scopes").map(String).filter(Boolean);
    if (!name || scopes.length === 0) {
      setProblem("A key needs a name and at least one scope.");
      return;
    }
    setBusy(true);
    const result = await submit<NewApiKey>("api_key_create", {
      body: { name, scopes, workspaceId },
    });
    setBusy(false);
    if (result.kind === "ready") {
      setProblem(null);
      setMinted(result.data);
      form.reset();
      reload();
      return;
    }
    setProblem("failure" in result ? result.failure.message : "The key was not minted.");
  }

  return (
    <Card
      title="API keys"
      description="Workspace keys. The contract records no last-used time, so none is shown."
    >
      <div className="stack">
        {problem ? (
          <Notice status="warning" title="That request was declined" live>
            <p className="small">{problem}</p>
          </Notice>
        ) : null}

        {minted ? (
          <Notice status="warning" title="Copy this key now" live>
            <p className="small">
              This value is returned once and never appears in a later read. Closing this notice
              discards it.
            </p>
            <p className="mono" style={{ wordBreak: "break-all" }}>{minted.value}</p>
            <p>
              <button type="button" className="button" onClick={() => setMinted(null)}>
                I have copied it
              </button>
            </p>
          </Notice>
        ) : null}

        <form className="stack" onSubmit={onSubmit}>
          <label className="field">
            <span>Key name</span>
            <input name="name" required maxLength={120} />
          </label>
          <fieldset className="field" style={{ border: 0, padding: 0, margin: 0 }}>
            <legend className="small muted">Scopes</legend>
            <div className="row">
              {COMMON_SCOPES.map((scope) => (
                <label key={scope} className="small row" style={{ gap: "var(--aex-space-1)" }}>
                  <input type="checkbox" name="scopes" value={scope} />
                  <span className="mono">{scope}</span>
                </label>
              ))}
            </div>
          </fieldset>
          <p>
            <button type="submit" className="button" data-variant="primary" disabled={busy}>
              {busy ? "Minting…" : "Mint key"}
            </button>
          </p>
        </form>

        <Resolved state={state} reload={reload} billingHref={billingHref}>
          {(page) =>
            page.items.length === 0 ? (
              <Empty title="No keys in this workspace." hint="A workspace is created without one." />
            ) : (
              <div className="scroller">
                <table>
                  <caption className="sr-only">Workspace API keys</caption>
                  <thead>
                    <tr>
                      <th scope="col">Name</th>
                      <th scope="col">Scopes</th>
                      <th scope="col">Created</th>
                      <th scope="col">State</th>
                      <th scope="col"><span className="sr-only">Actions</span></th>
                    </tr>
                  </thead>
                  <tbody>
                    {page.items.map((key) => (
                      <tr key={key.id}>
                        <td className="small">{key.name}</td>
                        <td className="mono small muted">{key.scopes.join(" ")}</td>
                        <td className="small muted">{instant(key.createdAt)}</td>
                        <td>
                          {key.revokedAt
                            ? <Badge status="warning" label="Revoked" />
                            : <Badge status="good" label="Active" />}
                        </td>
                        <td>
                          {key.revokedAt ? null : (
                            <button
                              type="button"
                              className="button"
                              onClick={() => {
                                void submit("api_key_revoke", { parameters: { apiKeyId: key.id } })
                                  .then((result) => {
                                    if (result.kind === "ready") reload();
                                    else if ("failure" in result) setProblem(result.failure.message);
                                  });
                              }}
                            >
                              Revoke
                            </button>
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
      </div>
    </Card>
  );
}
