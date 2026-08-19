# Returning unused credit

Refunds are a support operation, not a customer dashboard control. Use the AEX operator endpoint
so the Stripe refund and the spendable-credit ledger stay consistent. Do not create the refund
directly in the Stripe Dashboard: Stripe would return the money while AEX left the credit
spendable.

## Before the request

1. Confirm the request came from the account email and identify the paid top-up. Its Stripe
   Checkout Session has `aex_topup_id` metadata; the customer can also read the ID from
   `GET /v1/topups` with their account token.
2. Agree the amount of unused credit to return. It cannot exceed either the unrefunded part of
   that top-up or the account's current metered balance.
3. Wait for any running session turn to finish. The endpoint sweeps usage before reserving credit
   and refuses the request while a turn is open.

## Create or retry the refund

Generate one random request key and keep it with the support record. Reuse that exact key and
body after a timeout or `payment_error`; never invent a second key merely because the first HTTP
response was uncertain.

```sh
curl --fail-with-body https://api.aex.dev/v1/admin/refunds \
  --request POST \
  --header "Authorization: Bearer $AEX_OPERATOR_TOKEN" \
  --header "Idempotency-Key: refund-<random-uuid>" \
  --header "Content-Type: application/json" \
  --data '{"topup_id":"top_...","amount_cents":1000}'
```

The response status is:

- `succeeded`: Stripe completed the refund and the credit remains removed from the AEX balance.
- `pending`: the credit is reserved and cannot be spent. Retry the same request later to
  reconcile Stripe's latest state.
- `failed`: Stripe definitively rejected or failed the refund and AEX restored the reserved
  credit. Fix the cause, then make a new intended attempt with a new request key.

AEX sends the local refund ID to Stripe as metadata and checks for it before every create. This
recovers safely even if the service stopped after Stripe accepted the refund or the retry happens
after Stripe's idempotency-key retention window.
