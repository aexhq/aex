# aex

Session-oriented developer platform for running agents. A session = one agent, one workspace,
one optional sandbox (**hand**), any number of messages over any lifetime. BYOK — you bring the
model key. The **brain** (LLM harness) runs as a long-lived service; untrusted code runs only in
the hand, a microVM.

This repository is the public contract and control-plane home:

| Directory | Holds |
| --- | --- |
| `contracts/` | JSON Schema for the brain↔hand ABI v1, the session API v1 and the control API v1 (+ OpenAPI), the sealed tool manifest, worked examples — **the source of truth** |
| `crates/aex-contracts` | generated Rust types + the shared hashes (`manifest_digest`, `call_hash`) |
| `crates/aex-control` | the control plane: identity (accounts, API keys), prepaid billing (signed Stripe webhooks plus poll recovery, or a loud-bannered fake), session authority (the session API served verbatim in front of a brain, with admission), rated compute/storage/search usage folded from the session event log |
| `crates/aex-sdk` | Rust client SDK over the generated types |
| `crates/aex-cli` | the experimental `aex` command line; it is not part of the Founding Beta onboarding path |
| `packages/contracts` | generated TypeScript types (`@aex/contracts`) + the same hashes |
| `tools/` | `gen.sh` regenerates everything derived from `contracts/`; CI fails on drift. `m1.sh` starts the real-wire gate stack (local brain + control plane + Stripe) |
| `docs/` | repository-level docs (the architecture record lives in `aex-research/docs/` until it moves here) |

The `brain` and `hands` implementations live in their own repositories and consume this one by tag.

## Using the Founding Beta

Join the waitlist at [aex.dev](https://aex.dev). After an invitation, the dashboard handles
signup, prepaid credit, and creation of a session API key. Continue with the
[API quickstart](docs/quickstart.md) to create a session, send its first turn, follow events,
and release compute without deleting the workspace.

The [control API](contracts/control/v1/openapi.yaml) and
[session API](contracts/session/v1/openapi.yaml) OpenAPI documents are the canonical public
interface. The CLI and official agent skill are post-MVP conveniences, not required for the
core flow.

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
npm test --workspace @aex/contracts           # tsc + ajv conformance
```

Requires: Rust 1.97 (`rust-toolchain.toml`), `cargo install cargo-typify`, Node ≥ 22, Python 3.

License: Apache-2.0.
