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
| `crates/aex-control` | the control plane: identity (accounts, API keys), prepaid billing (Stripe or a loud-bannered fake), session authority (the session API served verbatim in front of a brain, with admission), rated usage folded from the session event log |
| `crates/aex-sdk` | Rust client SDK over the generated types |
| `crates/aex-cli` | the `aex` command line: signup / topup / keys / session / usage |
| `packages/contracts` | generated TypeScript types (`@aex/contracts`) + the same hashes |
| `tools/` | `gen.sh` regenerates everything derived from `contracts/`; CI fails on drift. `m1.sh` starts the real-wire gate stack (local brain + control plane + Stripe) |
| `docs/` | repository-level docs (the architecture record lives in `aex-research/docs/` until it moves here) |

The `brain` and `hands` implementations live in their own repositories and consume this one by tag.

## Running it

```
# a brain (github.com/aexhq/brain), local mode:
AEX_API_TOKEN=<token> brain
# the control plane in front of it (payments default to a loud fake; set AEX_PAYMENTS=stripe
# and STRIPE_SECRET_KEY for real billing):
AEX_BRAIN_TOKEN=<token> aex-control
# the stranger flow:
aex signup --email you@example.com
aex topup --usd 10
aex keys create --name laptop
aex session new --provider anthropic --model claude-sonnet-5
aex session send <ses_...> "run the tests"
aex usage
```

## Working on it

```
tools/gen.sh                                  # regenerate Rust + TS + examples + digest
cargo test && cargo clippy --all-targets -- -D warnings
npm test --workspace @aex/contracts           # tsc + ajv conformance
```

Requires: Rust 1.97 (`rust-toolchain.toml`), `cargo install cargo-typify`, Node ≥ 22, Python 3.

License: Apache-2.0.
