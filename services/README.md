# `services/`

Deployable API and long-lived service binaries. Each directory is one
independently publishable artifact with one composition root, one configuration
schema and one companion package under `tests/live/`.

`stripe-command-edge` and `stripe-webhook-edge` are TypeScript Lambda edges, not
Cargo members.
