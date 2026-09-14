# Credits and metered hosting

Aex accounts belong to developers and organizations. Application end users authenticate in the
application's own identity system. Aex does not create a second wallet for each application user.
Model credentials supplied by customers are billed by their model provider.

Existing accounts remain in preview until an account credential accepts an offered pricebook.
Omitting `billing` from the server configuration offers no prepaid enrollment or checkout. Configuring
a pricebook does not migrate existing accounts. Stripe is optional; when configured, its secret and
webhook credentials must match the selected `test` or `live` mode or startup fails.

## Amounts, prices and limits

Amounts use integer micro-USD: 1 USD is 1,000,000 micro-USD. Checkout and refund amounts use integer
USD cents. A published pricebook has an immutable ID and rational rates `{micro_usd, units}` for
`turn_ms`, `sandbox_ms`, `attachment_byte_secs` and `egress_bytes`. To change prices, publish a new
ID and obtain account acceptance. Existing reservations retain their accepted pricebook.

For each resource, rating computes `floor(cumulative_units * micro_usd / units)` and debits only the
change since the previous observation. The account lock serializes capacity, credits, refunds and
admission. The append-only ledger is the balance history; reservations account for held funds.
Available credits equal balance minus reserved credits. Disputes may leave a negative balance.

Prepaid execution requires `x-aex-max-cost-micro-usd`. The SDK's `maxCostMicroUsd` client option sets
this ceiling for each admitted operation. A turn reserves its configured maximum billing duration
before dispatch; journaled start and terminal timestamps determine the charge, capped at that
approved duration. Cancellation or recovery time beyond the cap is absorbed by the service.
The monthly UTC spend limit includes usage already charged that month and all outstanding holds.
Lowering a limit stops further admission when these commitments exceed the new limit.

For attachments, also provide `x-aex-download-budget-bytes` (`downloadBudgetBytes` in the SDK).
The upload ceiling covers both the retained byte-seconds and the entire download allowance.
Concurrent downloads cannot exceed that allowance. HEAD is free. Bytes read from S3 for delivery
are metered cumulatively, including a partial transfer before disconnection. A transfer's unused
byte claim expires at its finite deadline. Storage starts at publication; unpublished objects are
cleaned without charging storage. Confirmed object deletion closes both holds and releases their
remaining credits. Failed deletion retains the holds. The approved storage charge is capped even
if infrastructure cleanup completes late.

## Account API

| Method | Route | Credential |
| --- | --- | --- |
| GET | `/v1/billing` | Account session or workload key |
| PUT | `/v1/billing` | Account session; `{pricebook, spend_limit_micro_usd}` |
| GET | `/v1/billing/ledger?before=ID` | Account session or workload key; 100 entries per page |
| GET | `/v1/billing/topups`, `/v1/billing/refunds` | Account session or workload key; latest 100 |
| POST | `/v1/billing/topups` | Account session, operation key, `{amount_cents}` |
| POST | `/v1/billing/refunds` | Account session, operation key, `{topup, amount_cents}` |
| POST | `/v1/billing/sync` | Account session; retrieve and reconcile an existing Stripe object |
| POST | `/v1/webhooks/stripe` | Stripe signature over the raw body |

The SDK exposes these operations under `aex.billing`. CLI equivalents include `aex billing`,
`aex billing ledger`, `aex billing set PRICEBOOK MICRO_USD`, `aex billing topup CENTS KEY`, and
`aex billing refund TOPUP CENTS KEY`. New payments require an account credential; workload keys
cannot accept prices, change spend limits, top up or refund.

Topup and refund keys identify durable payment intents. A repeated key with different input conflicts.
An uncertain creation stays unresolved and is not resent, including after Stripe's idempotency
retention window. Sync accepts `{kind: "topup", id, checkout_id?}` or
`{kind: "refund", id, refund_id?}`. A supplied provider reference must match the intent's metadata,
account, amount and any already recorded reference. Sync never creates a payment. It also refreshes
receipt links after checkout. The browser's return URL is not proof of payment.

## Stripe and recovery

Configure `STRIPE_SECRET_KEY` and `STRIPE_WEBHOOK_SECRET` outside source control. Subscribe to
`checkout.session.completed`, `checkout.session.async_payment_succeeded`,
`checkout.session.async_payment_failed`, `checkout.session.expired`, `refund.created`,
`refund.updated`, `refund.failed`, `charge.dispute.funds_withdrawn`, and
`charge.dispute.funds_reinstated`. The webhook remains available while customer admission is drained.
Use the Checkout API version pinned in `payments.rs` for snapshot events.

Credit is issued only for a matching paid USD Checkout, once per topup. Pending refunds retain
their credit hold. Success debits the wallet once; failure or cancellation releases the hold.
Out-of-order nonterminal callbacks cannot reverse a terminal refund. Disputed withdrawals suspend
new billing admission; reinstated funds restore the balance once. Money mutations have their own
stable identities, so a crash before recording webhook completion does not duplicate them.

`record_metered_usage` is a private operator command; customer keys cannot submit costs or quantities.
`adjust_credits` appends a referenced correction with a required reason. Never edit ledger rows.
Preserve the product database with Brain's matching journals during backup and restore. Reconcile
Stripe payments, refunds, disputes and later adjustments before reopening a restored wallet.

Relevant provider contracts: [Checkout fulfillment](https://docs.stripe.com/checkout/fulfillment),
[webhook signatures](https://docs.stripe.com/webhooks),
[refund states](https://docs.stripe.com/api/refunds/object), and
[idempotent requests](https://docs.stripe.com/api/idempotent_requests).
