"use client";

import { useState, type FormEvent } from "react";
import type {
  BillingBalance,
  BillingTransactionPage,
  BillingUsagePage,
  HostedSession,
  PaymentMethodPage,
} from "@aexhq/sdk";

import { DEADLINE_MS, submit, useResource } from "../client";
import { Card, Empty, Notice, Resolved } from "../components";
import { centsToUsd } from "../panel";
import { instant, label } from "../status";

export function BillingPanel() {
  const balance = useResource<BillingBalance>("billing_balance_get", { deadlineMs: DEADLINE_MS.control });
  const cards = useResource<PaymentMethodPage>("billing_payment_methods_list", { deadlineMs: DEADLINE_MS.control });
  const transactions = useResource<BillingTransactionPage>("billing_transactions_list", {
    parameters: { limit: "50" }, deadlineMs: DEADLINE_MS.resources,
  });
  const usage = useResource<BillingUsagePage>("billing_usage_get", {
    parameters: { limit: "100" }, deadlineMs: DEADLINE_MS.resources,
  });
  const [hosted, setHosted] = useState<HostedSession | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  async function open(routeId: "billing_top_up_checkout_create" | "billing_payment_method_session_create", body: unknown) {
    const result = await submit<HostedSession>(routeId, { body });
    if (result.kind === "ready") {
      setProblem(null);
      setHosted(result.data);
    } else {
      setProblem("failure" in result ? result.failure.message : "The hosted page could not be opened.");
    }
  }

  function topUp(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const dollars = Number(new FormData(event.currentTarget).get("amount"));
    if (Number.isFinite(dollars) && dollars > 0) {
      void open("billing_top_up_checkout_create", { amountCents: String(Math.round(dollars * 100)) });
    }
  }

  return (
    <div className="stack">
      {problem ? <Notice status="warning" title="Billing action failed" live><p>{problem}</p></Notice> : null}
      {hosted ? <Notice status="good" title="Continue with Stripe" live>
        <a className="button" data-variant="primary" href={hosted.url}>Open secure page</a>
      </Notice> : null}
      <Card title="Balance" actions={<button className="button" onClick={balance.reload}>Refresh</button>}>
        <Resolved state={balance.state} reload={balance.reload}>{(value) => <div className="grid">
          <div className="stat"><span className="stat-label">Available</span><span className="stat-value">{centsToUsd(value.availableCents)}</span></div>
          <div className="stat"><span className="stat-label">Reserved</span><span className="stat-value">{centsToUsd(value.reservedCents)}</span></div>
          <div className="stat"><span className="stat-label">Pending</span><span className="stat-value">{centsToUsd(value.pendingCents)}</span></div>
        </div>}</Resolved>
        <form className="row" onSubmit={topUp}>
          <label className="field"><span>Top up (USD)</span><input name="amount" type="number" min="1" defaultValue="50" required /></label>
          <button className="button" data-variant="primary">Continue to checkout</button>
        </form>
      </Card>
      <Card title="Cards" actions={<button className="button" onClick={() => void open("billing_payment_method_session_create", { consent: true })}>Add card</button>}>
        <Resolved state={cards.state} reload={cards.reload}>{(page) => page.items.length === 0 ? <Empty title="No cards." /> : <ul className="stack-tight">
          {page.items.map((card) => <li key={card.id} className="row"><span>{label(card.brand)} •••• {card.last4}</span><span className="muted">expires {card.expiryMonth}/{card.expiryYear}</span><span className="spacer" />
            <button className="button" onClick={() => void submit("billing_payment_method_delete", { parameters: { paymentMethodId: card.id } }).then(cards.reload)}>Remove</button>
          </li>)}
        </ul>}</Resolved>
      </Card>
      <Card title="Transactions" flush><Resolved state={transactions.state} reload={transactions.reload}>{(page) => page.items.length === 0 ? <Empty title="No transactions." /> : <table><tbody>{page.items.map((row) => <tr key={row.id}><td>{instant(row.occurredAt)}</td><td>{label(row.kind)}</td><td>{row.direction === "credit" ? "+" : "−"}{centsToUsd(row.amountCents)}</td><td>{centsToUsd(row.balanceAfterCents)}</td></tr>)}</tbody></table>}</Resolved></Card>
      <Card title="Usage" description="Rated usage, including zero-dollar BYOK model-token observability." flush><Resolved state={usage.state} reload={usage.reload}>{(page) => page.items.length === 0 ? <Empty title="No usage." /> : <table><tbody>{page.items.map((row, index) => <tr key={`${row.category}-${row.serviceTime.gte}-${index}`}><td>{label(row.category)}</td><td>{row.quantity} {row.unit}</td><td>{centsToUsd(row.amountCents)}</td><td>{label(row.settlement)}</td></tr>)}</tbody></table>}</Resolved></Card>
    </div>
  );
}
