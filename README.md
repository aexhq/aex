# aex

The session backend for AI apps. Start a durable agent session, give it tools, and get back text or
validated data. Every session includes an automatically managed computer. BYOK — you bring the
model key.

This repository is the public contract and control-plane home:

| Directory | Holds |
| --- | --- |
| `contracts/` | JSON Schema for the brain↔hand ABI v1, the session API v1 and the control API v1 (+ OpenAPI), the sealed tool manifest, worked examples — **the source of truth** |
| `crates/aex-contracts` | generated Rust types + the shared hashes (`manifest_digest`, `call_hash`) |
| `crates/aex-control` | the control plane: identity (accounts, API keys), prepaid billing (signed Stripe webhooks, poll recovery, and operator refunds), session authority (the session API served verbatim in front of a brain, with admission), rated compute/storage/search usage folded from the session event log |
| `crates/aex-sdk` | Rust client SDK over the generated types |
| `crates/aex-cli` | the existing internal Rust diagnostic CLI |
| `packages/contracts` | generated TypeScript types (`@aexhq/contracts`) + the same hashes |
| `packages/sdk` | the public TypeScript SDK (`@aexhq/sdk`) |
| `packages/cli` | the public Node CLI (`@aexhq/cli`, binary `aex`) |
| `tools/` | `gen.sh` regenerates everything derived from `contracts/`; CI fails on drift. `m1.sh` starts the real-wire gate stack (local brain + control plane + Stripe) |
| `docs/` | repository-level docs (the architecture record lives in `aex-research/docs/` until it moves here) |

The `brain` and `hands` implementations live in their own repositories and consume this one by tag.

## Using the beta

Join the waitlist at [aex.dev](https://aex.dev). After an invitation, the dashboard handles
signup, prepaid credit, and creation of a session API key. Continue with the short
[SDK quickstart](docs/quickstart.md) to create a session and await typed output.

The [control API](contracts/control/v1/openapi.yaml) and
[session API](contracts/session/v1/openapi.yaml) OpenAPI documents are the canonical public
interface. The SDK is the onboarding surface; raw HTTP remains available as reference.

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
tools/gen.sh                                  # regenerate Rust + TS + examples + digest
cargo test && cargo clippy --all-targets -- -D warnings
npm test --workspace @aexhq/contracts         # tsc + ajv conformance
npm test --workspace @aexhq/sdk               # SDK contract + event replay tests
```

Requires: Rust 1.97 (`rust-toolchain.toml`), `cargo install cargo-typify`, Node ≥ 22, Python 3.

License: Apache-2.0.
