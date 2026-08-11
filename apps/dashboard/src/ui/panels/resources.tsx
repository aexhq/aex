"use client";

import { useState, type FormEvent } from "react";
import { DEADLINE_MS, submit, useResource } from "../client";
import { Badge, Card, Empty, Notice, Resolved } from "../components";
import type { PanelState } from "../panel";
import { bytes, instant, label } from "../status";
import type {
  DownloadGrant,
  EffectiveWorkspaceLimit,
  LimitValue,
  Page,
  ProviderCredential,
  RegisteredEntry,
} from "../wire";

interface Scope {
  readonly region: string;
  readonly billingHref?: string | undefined;
}

const CREDENTIAL_NAME = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;

/**
 * Dedicated BYOK provider credentials.
 *
 * Values are write-only on the wire and are never read back, so there is no reveal
 * affordance and no masked field pretending to hold one. This is deliberately
 * provider-specific rather than a generic secret store.
 */
export function ProviderCredentialsPanel({ region, billingHref }: Scope) {
  const { state, reload } = useResource<Page<ProviderCredential>>("provider_credentials_list", {
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
    const provider = String(data.get("provider") ?? "");
    const apiKey = String(data.get("apiKey") ?? "");
    if (!CREDENTIAL_NAME.test(name) || provider.length === 0 || apiKey.length < 8) {
      setProblem("A credential needs a valid name, provider, and API key of at least eight characters.");
      return;
    }
    setBusy(true);
    const result = await submit<ProviderCredential>("provider_credential_register", {
      region,
      body: { name, provider, apiKey },
    });
    form.reset();
    await act(result);
  }

  return (
    <Card title="Provider credentials" description="Dedicated BYOK bindings. Key material is never readable after registration.">
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
            <span>Provider</span>
            <input name="provider" required maxLength={128} />
          </label>
          <label className="field">
            <span>API key</span>
            <input name="apiKey" required minLength={8} maxLength={8192} type="password" autoComplete="off" />
          </label>
          <button type="submit" className="button" data-variant="primary" disabled={busy}>
            {busy ? "Saving…" : "Register credential"}
          </button>
        </form>

        <Resolved state={state} reload={reload} billingHref={billingHref}>
          {(page) =>
            page.items.length === 0 ? (
              <Empty title="No provider credentials in this workspace." />
            ) : (
              <div className="scroller">
                <table>
                  <caption className="sr-only">Provider credentials</caption>
                  <thead>
                    <tr>
                      <th scope="col">Name</th>
                      <th scope="col">Provider</th>
                      <th scope="col">State</th>
                      <th scope="col">Updated</th>
                      <th scope="col"><span className="sr-only">Actions</span></th>
                    </tr>
                  </thead>
                  <tbody>
                    {page.items.map((credential) => (
                      <tr key={credential.id}>
                        <td className="mono small">{credential.name}</td>
                        <td className="small">{credential.provider}</td>
                        <td>
                          <Badge
                            status={credential.state === "ready" ? "good" : "warning"}
                            label={label(credential.state)}
                          />
                        </td>
                        <td className="small muted">{instant(credential.updatedAt)}</td>
                        <td>
                          {credential.state === "ready" ? (
                            <button
                              type="button"
                              className="button"
                              onClick={() => {
                                setBusy(true);
                                void submit("provider_credential_revoke", {
                                  region,
                                  parameters: { providerCredentialId: credential.id },
                                }).then(act);
                              }}
                            >
                              Revoke
                            </button>
                          ) : null}
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

/**
 * Durable opaque workspace files. Skills, tools, MCP configuration, custom
 * instructions, and arbitrary assets all use this one registry.
 */
export function RegistriesPanel({ region, billingHref }: Scope) {
  const { state, reload } = useResource<Page<RegisteredEntry>>("registry_files_list", {
    region,
    parameters: { limit: "100" },
    deadlineMs: DEADLINE_MS.resources,
  });
  const [grant, setGrant] = useState<DownloadGrant | null>(null);

  return (
    <Card
      title="Registered resources"
      description="Opaque workspace files a session may mount, including AGENTS.md, skills, tools, MCP configuration, and assets."
      flush
    >
      <Resolved state={state} reload={reload} billingHref={billingHref}>
        {(page) =>
          page.items.length === 0 ? (
            <Empty title="No registered files." />
          ) : (
            <>
              {grant ? (
                <div style={{ padding: "var(--aex-space-4)" }}>
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
                  <caption className="sr-only">Registered files</caption>
                  <thead>
                    <tr>
                      <th scope="col">Name</th>
                      <th scope="col">Digest</th>
                      <th scope="col">Size</th>
                      <th scope="col">Updated</th>
                      <th scope="col"><span className="sr-only">Actions</span></th>
                    </tr>
                  </thead>
                  <tbody>
                    {page.items.map((entry) => (
                      <tr key={entry.name}>
                        <td className="mono small">{entry.name}</td>
                        <td className="mono small muted">{entry.sha256.slice(0, 12)}…</td>
                        <td className="numeric">{bytes(entry.sizeBytes)}</td>
                        <td className="small muted">{instant(entry.updatedAt)}</td>
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
