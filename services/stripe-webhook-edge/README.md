# `stripe-webhook-edge`

Minimal TypeScript Lambda that verifies raw Stripe webhook signatures and
normalizes the payload.

Target: it returns 2xx only after an accepted durable handoff to
`finance-ingest`. Malformed input fails boundedly; transient ingest failure
relies on Stripe redelivery.

Not this deployable's job: any database role, any finance transition, or any
business decision about the event it forwards.

Implementation is owned by the finance stream. This package is not a Cargo
workspace member.
