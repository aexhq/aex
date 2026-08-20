# Aex

The session backend for AI apps. Start a durable agent session, give it tools, and get back text or
validated data. Every session includes an automatically managed computer. BYOK — you bring the
model key.

This repository is the public contract and control-plane home:

| Directory | Holds |
| --- | --- |
| `contracts/` | Aex-only account, billing, and control-plane JSON Schema, OpenAPI, and worked examples |
| `crates/aex-contracts` | generated Rust types for Aex-owned control-plane contracts |
| `crates/aex-control` | the control plane: identity (accounts, API keys), prepaid billing (signed Stripe webhooks, poll recovery, and operator refunds), session authority (the session API served verbatim in front of a brain, with admission), rated compute/storage/search usage folded from the session event log |
| `crates/aex-sdk` | Rust client SDK over the generated types |
| `crates/aex-cli` | the existing internal Rust diagnostic CLI |
| `packages/contracts` | generated TypeScript types for Aex-owned contracts (`@aexhq/contracts`) |
| `packages/sdk` | the public TypeScript SDK (`@aexhq/sdk`) |
| `packages/tools` | individual built-in tool selections (`@aexhq/tools`) |
| `packages/cli` | the public Node CLI (`@aexhq/cli`, binary `aex`) |
| `tools/` | `gen.sh` regenerates everything derived from `contracts/`; CI fails on drift. `m1.sh` starts the real-wire gate stack (local brain + control plane + Stripe) |
| `docs/` | repository-level docs (the architecture record lives in `aex-research/docs/` until it moves here) |

The independent `brain` repository owns the public session API and Brain↔Hand protocol. `hands`
implements Brain's Hand interface; Aex consumes both at immutable source identities.

## Using the alpha

Join the waitlist at [aex.dev](https://aex.dev). After an invitation, the dashboard handles
signup, prepaid credit, and creation of a session API key. Continue with the short
[SDK quickstart](docs/quickstart.md) to create a session and await typed output.

The [control API](contracts/control/v1/openapi.yaml) is owned here; the
[session API](https://github.com/aexhq/brain/blob/main/contracts/session/v1/openapi.yaml) is owned
by Brain. The SDK is the onboarding surface; raw HTTP remains available as reference.

## Running the services locally

```
# a brain (github.com/aexhq/brain), local mode:
AEX_API_TOKEN=<token> brain
# the control plane in front of it (payments default to a loud fake; set AEX_PAYMENTS=stripe
# and STRIPE_SECRET_KEY + STRIPE_WEBHOOK_SECRET for real billing):
AEX_BRAIN_TOKEN=<token> aex-control
```

Managed `web_search` is available when the brain has `SERPER_API_KEY`; successful committed
search results are rated at the public per-query rate. `web_fetch` uses guarded outbound HTTPS
with private/link-local/metadata destinations rejected at every redirect.

## Working on it

```
tools/gen.sh                                  # regenerate Aex-owned Rust + TS + examples
cargo test && cargo clippy --all-targets -- -D warnings
npm test --workspace @aexhq/contracts         # tsc + ajv conformance
npm test --workspace @aexhq/sdk               # SDK contract + event replay tests
npm test --workspace @aexhq/tools             # official tool selection + SDK integration tests
```

Requires: Rust 1.97 (`rust-toolchain.toml`), `cargo install cargo-typify`, Node ≥ 22, Python 3.

License: Apache-2.0.
