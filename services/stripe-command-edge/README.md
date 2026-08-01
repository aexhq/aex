# `stripe-command-edge`

Minimal TypeScript Lambda that speaks the official pinned Stripe API protocol.

Target: it executes only an already-admitted effect identity and idempotency key
prepared by `finance-api`. A timeout becomes an unknown provider outcome that
`finance-reconcile` resolves; it never retries blindly.

Not this deployable's job: any database role, any AEX authority, rating,
posting, or deciding that a charge should happen.

Implementation is owned by the finance stream. This package is not a Cargo
workspace member.
