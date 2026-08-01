"use client";

import { useState, type FormEvent } from "react";
import type { RouteId } from "@aexhq/sdk";

import { DEADLINE_MS, submit, useResource } from "../client";
import { Badge, Card, Empty, Notice, Resolved } from "../components";
import type { PanelState } from "../panel";
import { bytes, instant, label } from "../status";
import type {
  DownloadGrant,
  EffectiveWorkspaceLimit,
  LimitValue,
  Page,
  RegisteredEntry,
  SecretMetadata,
} from "../wire";

interface Scope {
  readonly region: string;
  readonly billingHref?: string | undefined;
}

const SECRET_NAME = /^[A-Za-z0-9._-]{1,128}$/;

/**
 * Secrets.
 *
 * Values are write-only on the wire and are never read back, so there is no reveal
 * affordance and no masked field pretending to hold one. Delete and revoke are both
 * offered because they are not the same act: delete affects future admission,
 * revoke also cancels custody that is already held.
 */
export function SecretsPanel({ region, billingHref }: Scope) {
  const { state, reload } = useResource<Page<SecretMetadata>>("secrets_list", {
    region,
    parameters: { limit: "100" },
    deadlineMs: DEADLINE_MS.resources,
  });
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function act(result: PanelState<unknown>) {
    setBusy(false);
    if (result.kind === "ready") {
      setProblem(null);
      reload();
      return;
    }
    setProblem("failure" in result ? result.failure.message : "The request did not complete.");
  }

  async function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    const name = String(data.get("name") ?? "");
    const value = String(data.get("value") ?? "");
    if (!SECRET_NAME.test(name) || value.length === 0) {
      setProblem("A secret needs a name of letters, digits, dot, dash or underscore, and a value.");
      return;
    }
    setBusy(true);
    const result = await submit<SecretMetadata>("secret_put", {
      region,
      parameters: { name },
      body: { value },
    });
    form.reset();
    await act(result);
  }

  return (
    <Card title="Secrets" description="Names and states only. A value is never readable after it is set.">
      <div className="stack">
        {problem ? (
          <Notice status="warning" title="That change was not applied" live>
            <p className="small">{problem}</p>
          </Notice>
        ) : null}

        <form className="row" onSubmit={onSubmit}>
          <label className="field">
            <span>Name</span>
            <input name="name" required maxLength={128} pattern="[A-Za-z0-9._\-]+" />
          </label>
          <label className="field">
            <span>Value</span>
            <input name="value" required type="password" autoComplete="off" />
          </label>
          <button type="submit" className="button" data-variant="primary" disabled={busy}>
            {busy ? "Saving…" : "Set secret"}
          </button>
        </form>

        <Resolved state={state} reload={reload} billingHref={billingHref}>
          {(page) =>
            page.items.length === 0 ? (
              <Empty title="No secrets in this workspace." />
            ) : (
              <div className="scroller">
                <table>
                  <caption className="sr-only">Workspace secrets</caption>
                  <thead>
                    <tr>
                      <th scope="col">Name</th>
                      <th scope="col">State</th>
                      <th scope="col">Updated</th>
                      <th scope="col"><span className="sr-only">Actions</span></th>
                    </tr>
                  </thead>
                  <tbody>
                    {page.items.map((secret) => (
                      <tr key={secret.name}>
                        <td className="mono small">{secret.name}</td>
                        <td>
                          <Badge
                            status={secret.state === "ready" ? "good" : "warning"}
                            label={label(secret.state)}
                          />
                        </td>
                        <td className="small muted">{instant(secret.updatedAt)}</td>
                        <td>
                          <div className="row">
                            {secret.state === "ready" ? (
                              <button
                                type="button"
                                className="button"
                                onClick={() => {
                                  setBusy(true);
                                  void submit("secret_revoke", { region, parameters: { name: secret.name } }).then(act);
                                }}
                              >
                                Revoke
                              </button>
                            ) : null}
                            <button
                              type="button"
                              className="button"
                              onClick={() => {
                                setBusy(true);
                                void submit("secret_delete", { region, parameters: { name: secret.name } }).then(act);
                              }}
                            >
                              Delete
                            </button>
                          </div>
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

const REGISTRIES = [
  { key: "files", routeId: "registry_files_list" satisfies RouteId, title: "Files" },
  { key: "tools", routeId: "registry_tools_list" satisfies RouteId, title: "Tools" },
  { key: "skills", routeId: "registry_skills_list" satisfies RouteId, title: "Skills" },
  { key: "mcp", routeId: "registry_mcp_servers_list" satisfies RouteId, title: "MCP servers" },
  { key: "instructions", routeId: "registry_instructions_list" satisfies RouteId, title: "Instructions" },
] as const;

type RegistryKey = (typeof REGISTRIES)[number]["key"];

/**
 * The five registries are one panel, not five.
 *
 * Every registry entry has the same shape — name, digest, size, revision, times —
 * so five near-identical tables would be five copies of one design. Writing an
 * entry is not offered: each registry has its own typed value schema and the file
 * registry needs the staged-upload flow, none of which this surface can express
 * without inventing an editor per type. Recorded, not faked.
 */
export function RegistriesPanel({ region, billingHref }: Scope) {
  const [kind, setKind] = useState<RegistryKey>("files");
  const active = REGISTRIES.find((candidate) => candidate.key === kind) ?? REGISTRIES[0];
  const { state, reload } = useResource<Page<RegisteredEntry>>(active.routeId, {
    region,
    parameters: { limit: "100" },
    deadlineMs: DEADLINE_MS.resources,
  });
  const [grant, setGrant] = useState<DownloadGrant | null>(null);

  return (
    <Card
      title="Registered resources"
      description="What a session may mount. Entries are written from the SDK or the CLI."
      actions={
        <label className="field">
          <span className="sr-only">Registry</span>
          <select value={kind} onChange={(event) => setKind(event.target.value as RegistryKey)}>
            {REGISTRIES.map((candidate) => (
              <option key={candidate.key} value={candidate.key}>{candidate.title}</option>
            ))}
          </select>
        </label>
      }
      flush
    >
      <Resolved state={state} reload={reload} billingHref={billingHref}>
        {(page) =>
          page.items.length === 0 ? (
            <Empty title={`No registered ${active.title.toLowerCase()}.`} />
          ) : (
            <>
              {grant ? (
                <div style={{ padding: "var(--space-4)" }}>
                  <Notice status="good" title="Download ready" live>
                    <p className="small">
                      Signed for {bytes(grant.authorizedBytes)}, expires {instant(grant.expiresAt)}.
                    </p>
                    <p>
                      <a className="button" data-variant="primary" href={grant.url} rel="noreferrer">
                        Download
                      </a>
                    </p>
                  </Notice>
                </div>
              ) : null}
              <div className="scroller">
                <table>
                  <caption className="sr-only">Registered {active.title}</caption>
                  <thead>
                    <tr>
                      <th scope="col">Name</th>
                      <th scope="col">Digest</th>
                      <th scope="col">Size</th>
                      <th scope="col">Updated</th>
                      {active.key === "files" ? <th scope="col"><span className="sr-only">Actions</span></th> : null}
                    </tr>
                  </thead>
                  <tbody>
                    {page.items.map((entry) => (
                      <tr key={entry.name}>
                        <td className="mono small">{entry.name}</td>
                        <td className="mono small muted">{entry.sha256.slice(0, 12)}…</td>
                        <td className="numeric">{bytes(entry.sizeBytes)}</td>
                        <td className="small muted">{instant(entry.updatedAt)}</td>
                        {active.key === "files" ? (
                          <td>
                            <button
                              type="button"
                              className="button"
                              onClick={() => {
                                void submit<DownloadGrant>("registry_files_download_create", {
                                  region,
                                  parameters: { name: entry.name },
                                  body: {},
                                }).then((result) => {
                                  if (result.kind === "ready") setGrant(result.data);
                                });
                              }}
                            >
                              Get link
                            </button>
                          </td>
                        ) : null}
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

/**
 * Effective safety limits. Read-only because the contract has no public mutation
 * route for them — an editable field here would be a promise the API cannot keep.
 */
export function LimitsPanel({ region, billingHref }: Scope) {
  const { state, reload } = useResource<Page<EffectiveWorkspaceLimit>>("workspace_limits_list", {
    region,
    parameters: { limit: "100" },
    deadlineMs: DEADLINE_MS.resources,
  });

  return (
    <Card title="Effective limits" description="The ceilings in force. There is no public route to change them." flush>
      <Resolved state={state} reload={reload} billingHref={billingHref}>
        {(page) =>
          page.items.length === 0 ? (
            <Empty title="No limits reported." />
          ) : (
            <div className="scroller">
              <table>
                <caption className="sr-only">Effective workspace limits</caption>
                <thead>
                  <tr>
                    <th scope="col">Limit</th>
                    <th scope="col">Value</th>
                    <th scope="col">Source</th>
                    <th scope="col">Changed</th>
                  </tr>
                </thead>
                <tbody>
                  {page.items.map((limit) => (
                    <tr key={limit.id}>
                      <td className="small">{label(limit.id)}</td>
                      <td className="mono small">{describe(limit.effectiveValue)}</td>
                      <td>
                        <Badge
                          status={limit.source === "workspace_override" ? "warning" : undefined}
                          label={label(limit.source)}
                        />
                      </td>
                      <td className="small muted">{instant(limit.changedAt)}</td>
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

function describe(value: LimitValue): string {
  if (value.shape === "scalar") return value.value;
  return Object.entries(value.values).map(([key, entry]) => `${key}=${entry}`).join(" ");
}
