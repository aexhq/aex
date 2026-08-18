# aex

Session-oriented developer platform for running agents. A session = one agent, one workspace,
one optional sandbox (**hand**), any number of messages over any lifetime. BYOK — you bring the
model key. The **brain** (LLM harness) runs as a long-lived service; untrusted code runs only in
the hand, a microVM.

This repository is the public contract and control-plane home:

| Directory | Holds |
| --- | --- |
| `contracts/` | JSON Schema for the brain↔hand ABI v1 and the session API v1 (+ OpenAPI), the sealed tool manifest, worked examples — **the source of truth** |
| `crates/aex-contracts` | generated Rust types + the shared hashes (`manifest_digest`, `call_hash`) |
| `packages/contracts` | generated TypeScript types (`@aex/contracts`) + the same hashes |
| `tools/` | `gen.sh` regenerates everything derived from `contracts/`; CI fails on drift |
| `docs/` | repository-level docs (the architecture record lives in the `agentsession` repo until it moves here) |

Planned next (see the roadmap in `agentsession/docs/MVP-ROADMAP.md`): SDK + CLI generated from the
contracts, conformance suite, control-plane services (identity, prepaid billing, session authority).
The `brain` and `hands` implementations live in their own repositories and consume this one by tag.

## Working on it

```
tools/gen.sh                                  # regenerate Rust + TS + examples + digest
cargo test && cargo clippy --all-targets -- -D warnings
npm test --workspace @aex/contracts           # tsc + ajv conformance
```

Requires: Rust 1.97 (`rust-toolchain.toml`), `cargo install cargo-typify`, Node ≥ 22, Python 3.

License: Apache-2.0.
