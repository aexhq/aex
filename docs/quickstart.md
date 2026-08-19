# AEX Founding Beta quickstart

The dashboard is the onboarding surface. Use it to join the waitlist, accept an invitation,
create the account, add prepaid credit, and create a **Session API key**. The API key is shown
once and starts with `aex_sk_`; it runs sessions but cannot manage billing or create other keys.

This guide begins after those dashboard steps. It uses `curl` and `jq`; the same JSON shapes work
from any HTTP client.

## 1. Set credentials

Keep both secrets out of source control. AEX is BYOK, so the model provider continues to bill you
directly.

```sh
export AEX_API_URL=https://api.aex.dev
export AEX_API_KEY='aex_sk_...'
export OPENAI_API_KEY='sk-...'
```

## 2. Create a session

The model, tools, history, and workspace belong to this session identity. Replace the model name
if your provider account uses another supported model.

```sh
jq -n '{
  model: {
    provider: "openai",
    name: "gpt-5",
    api_key: env.OPENAI_API_KEY
  }
}' | curl --fail-with-body --silent --show-error \
  --request POST "$AEX_API_URL/v1/sessions" \
  --header "Authorization: Bearer $AEX_API_KEY" \
  --header "Content-Type: application/json" \
  --header "Idempotency-Key: quickstart-create-1" \
  --data-binary @- | tee /tmp/aex-session.json

export AEX_SESSION_ID="$(jq -r .id /tmp/aex-session.json)"
```

Use a new idempotency key for each intended create or message. Retrying the same request with the
same key is safe.

## 3. Send the first turn

```sh
jq -n --arg content 'Create hello.txt containing a short greeting.' \
  '{content: $content}' | curl --fail-with-body --silent --show-error \
  --request POST "$AEX_API_URL/v1/sessions/$AEX_SESSION_ID/messages" \
  --header "Authorization: Bearer $AEX_API_KEY" \
  --header "Content-Type: application/json" \
  --header "Idempotency-Key: quickstart-message-1" \
  --data-binary @-
```

The API returns `202 Accepted` after the turn is admitted and journaled. Follow the replayable
event stream to see model output and tool activity:

```sh
curl --no-buffer --fail-with-body \
  "$AEX_API_URL/v1/sessions/$AEX_SESSION_ID/events?after=0&follow=true" \
  --header "Authorization: Bearer $AEX_API_KEY"
```

Reconnect later with `after=<last event sequence>` or the `Last-Event-ID` header; committed events
are replayed before live events continue.

## 4. Release compute and continue later

End the active runtime when the current work is done. This cancels a running turn if necessary,
syncs the workspace, and releases compute while retaining the session state.

```sh
curl --fail-with-body --silent --show-error \
  --request POST "$AEX_API_URL/v1/sessions/$AEX_SESSION_ID/end" \
  --header "Authorization: Bearer $AEX_API_KEY"
```

Send another message to the same session ID when you are ready to continue. Delete the session
only when you intend to irreversibly remove its workspace, artifacts, and journal.

## Reference

- [Session API](../contracts/session/v1/openapi.yaml): sessions, turns, events, files, artifacts,
  suspend/end, and delete semantics.
- [Control API](../contracts/control/v1/openapi.yaml): waitlist, invited signup, keys, balance,
  top-ups, usage, the live rate card, and operator-only unused-credit refunds.
- [Operator refund runbook](operator-refunds.md): safely keep Stripe and AEX credit in sync.
- [Generated examples](../contracts/examples/): conforming request, response, and event bodies.

Founding Beta limits and prices are published on [aex.dev](https://aex.dev) and at
[`GET /v1/rates`](https://api.aex.dev/v1/rates). There is no uptime SLA during the beta.
