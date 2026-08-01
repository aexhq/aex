"use client";

import { useState, type FormEvent } from "react";

import { DEADLINE_MS, submit, useResource } from "../client";
import { Card, Empty, Notice, Resolved } from "../components";
import { centsToUsd } from "../panel";
import { instant } from "../status";
import type { AutoTopupPolicy, BillingBalance, DownloadGrant, HostedSession, Page, StatementSummary } from "../wire";

/**
 * The prepaid balance.
 *
 * Three tiles, three different facts the authority reports separately: what may be
 * spent, what is held against admitted work, and what is recorded but not settled.
 * None of them is derived from another, and none of them is repeated anywhere else
 * in the dashboard.
 */
export function BalancePanel({ organizationId }: { organizationId: string }) {
  const { state, reload } = useResource<BillingBalance>("billing_balance_get", {
    parameters: { organizationId },
    deadlineMs: DEADLINE_MS.control,
  });
  const [hosted, setHosted] = useState<HostedSession | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  async function open(routeId: "billing_top_up_checkout_create" | "billing_portal_session_create", body: unknown) {
    setProblem(null);
    const result = await submit<HostedSession>(routeId, { parameters: { organizationId }, body });
    if (result.kind === "ready") setHosted(result.data);
    else setProblem("failure" in result ? result.failure.message : "The hosted page could not be opened.");
  }

  return (
    <Card
      title="Balance"
      description="Prepaid, in USD. Work is admitted against what is available."
      actions={<button type="button" className="button" onClick={reload}>Refresh</button>}
    >
      <Resolved state={state} reload={reload}>
        {(balance) => (
          <div className="stack">
            <div className="grid">
              <div className="stat">
                <span className="stat-label">Available</span>
                <span className="stat-value">{centsToUsd(balance.availableCents)}</span>
                <span className="stat-note">Spendable now · revision {balance.revision}</span>
              </div>
              <div className="stat">
                <span className="stat-label">Reserved</span>
                <span className="stat-value">{centsToUsd(balance.reservedCents)}</span>
                <span className="stat-note">Held against admitted work</span>
              </div>
              <div className="stat">
                <span className="stat-label">Pending</span>
                <span className="stat-value">{centsToUsd(balance.pendingCents)}</span>
                <span className="stat-note">Recorded, not yet settled</span>
              </div>
            </div>
            <p className="small muted">Updated {instant(balance.updatedAt)}.</p>

            {problem ? (
              <Notice status="warning" title="That could not be opened" live>
                <p className="small">{problem}</p>
              </Notice>
            ) : null}

            {hosted ? (
              <Notice status="good" title="Continue on the hosted page" live>
                <p className="small">The link expires {instant(hosted.expiresAt)}.</p>
                <p>
                  <a className="button" data-variant="primary" href={hosted.url} rel="noreferrer">
                    Open
                  </a>
                </p>
              </Notice>
            ) : null}

            <TopUpForm onOpen={(amountCents) => void open("billing_top_up_checkout_create", { amountCents })} />

            <p>
              <button
                type="button"
                className="button"
                onClick={() => void open("billing_portal_session_create", {})}
              >
                Manage payment methods
              </button>
            </p>
          </div>
        )}
      </Resolved>
    </Card>
  );
}

function TopUpForm({ onOpen }: { onOpen: (amountCents: string) => void }) {
  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const dollars = Number(new FormData(event.currentTarget).get("amount"));
    if (!Number.isFinite(dollars) || dollars <= 0) return;
    onOpen(String(Math.round(dollars * 100)));
  }
  return (
    <form className="row" onSubmit={onSubmit}>
      <label className="field">
        <span>Top up (USD)</span>
        <input name="amount" type="number" min="1" step="1" defaultValue="50" required />
      </label>
      <button type="submit" className="button" data-variant="primary">Continue to checkout</button>
    </form>
  );
}

export function AutoTopupPanel({ organizationId }: { organizationId: string }) {
  const { state, reload } = useResource<AutoTopupPolicy>("billing_auto_topup_policy_get", {
    parameters: { organizationId },
    deadlineMs: DEADLINE_MS.control,
  });
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    const enabled = data.get("enabled") === "on";
    const amount = Number(data.get("amount"));
    const threshold = Number(data.get("threshold"));
    if (!Number.isFinite(amount) || !Number.isFinite(threshold)) return;
    setBusy(true);
    // The contract says every field of the policy is required, so this replaces the
    // whole policy rather than patching a field.
    const result = await submit<AutoTopupPolicy>("billing_auto_topup_policy_put", {
      parameters: { organizationId },
      body: {
        enabled,
        amountCents: String(Math.round(amount * 100)),
        thresholdCents: String(Math.round(threshold * 100)),
      },
    });
    setBusy(false);
    if (result.kind === "ready") {
      setProblem(null);
      reload();
      return;
    }
    setProblem("failure" in result ? result.failure.message : "The policy was not replaced.");
  }

  return (
    <Card title="Automatic top-up" description="Replaces the whole policy; every field is required.">
      <Resolved state={state} reload={reload}>
        {(policy) => (
          <form className="stack" onSubmit={onSubmit}>
            {problem ? (
              <Notice status="warning" title="That policy was not accepted" live>
                <p className="small">{problem}</p>
              </Notice>
            ) : null}
            <label className="row small" style={{ gap: "var(--aex-space-2)" }}>
              <input type="checkbox" name="enabled" defaultChecked={policy.enabled} />
              <span>Top up automatically</span>
            </label>
            <div className="row">
              <label className="field">
                <span>When the balance falls to (USD)</span>
                <input
                  name="threshold"
                  type="number"
                  min="0"
                  step="1"
                  defaultValue={Number(policy.thresholdCents) / 100}
                  required
                />
              </label>
              <label className="field">
                <span>Add (USD)</span>
                <input
                  name="amount"
                  type="number"
                  min="1"
                  step="1"
                  defaultValue={Number(policy.amountCents) / 100}
                  required
                />
              </label>
            </div>
            <p>
              <button type="submit" className="button" disabled={busy}>
                {busy ? "Saving…" : "Replace policy"}
              </button>
            </p>
            <p className="small muted">Revision {policy.revision}, updated {instant(policy.updatedAt)}.</p>
          </form>
        )}
      </Resolved>
    </Card>
  );
}

export function StatementsPanel({ organizationId }: { organizationId: string }) {
  const { state, reload } = useResource<Page<StatementSummary>>("billing_statements_list", {
    parameters: { organizationId, limit: "24" },
    deadlineMs: DEADLINE_MS.resources,
  });
  const [grant, setGrant] = useState<DownloadGrant | null>(null);

  return (
    <Card title="Statements" description="Issued and immutable, newest first." flush>
      <Resolved state={state} reload={reload}>
        {(page) =>
          page.items.length === 0 ? (
            <Empty title="No statements have been issued yet." />
          ) : (
            <>
              {grant ? (
                <div style={{ padding: "var(--aex-space-4)" }}>
                  <Notice status="good" title="Statement ready" live>
                    <p className="small">The link expires {instant(grant.expiresAt)}.</p>
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
                  <caption className="sr-only">Issued statements</caption>
                  <thead>
                    <tr>
                      <th scope="col">Issued</th>
                      <th scope="col">Period</th>
                      <th scope="col">Total</th>
                      <th scope="col">Artifact</th>
                      <th scope="col"><span className="sr-only">Actions</span></th>
                    </tr>
                  </thead>
                  <tbody>
                    {page.items.map((statement) => (
                      <tr key={statement.id}>
                        <td className="small muted">{instant(statement.issuedAt)}</td>
                        <td className="small muted">
                          {instant(statement.period.gte)} — {instant(statement.period.lt)}
                        </td>
                        <td className="numeric">{centsToUsd(statement.totalCents)}</td>
                        <td className="mono small muted">{statement.artifactHash.slice(0, 12)}…</td>
                        <td>
                          <button
                            type="button"
                            className="button"
                            onClick={() => {
                              void submit<DownloadGrant>("billing_statement_download_create", {
                                parameters: { organizationId, statementId: statement.id },
                                body: {},
                              }).then((result) => {
                                if (result.kind === "ready") setGrant(result.data);
                              });
                            }}
                          >
                            Download
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
