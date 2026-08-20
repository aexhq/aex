# Aex

The session backend for AI applications. Create a durable agent session, grant the tools it needs,
and receive text or validated data. Every session includes a managed computer; model access is
bring-your-own-key.

[Join the alpha](https://aex.dev) · [TypeScript quickstart](docs/quickstart.md) ·
[Session API](https://github.com/aexhq/brain/blob/main/contracts/session/v1/openapi.yaml) ·
[Control API](contracts/control/v1/openapi.yaml)

## Repository

| Path | Purpose |
| --- | --- |
| `contracts/` | Account, billing, and control-plane schemas, OpenAPI, and examples |
| `crates/aex-control` | Identity, prepaid billing, session admission, and usage rating |
| `crates/aex-brain` | Hosted composition of Brain, Hands, and Aex capabilities |
| `packages/contracts` | Generated TypeScript control-plane types |
| `packages/sdk` | Public TypeScript SDK, published as `@aexhq/sdk` |
| `packages/tools` | Explicit built-in tool selections, published as `@aexhq/tools` |
| `packages/cli` | Public Node.js CLI, published as `@aexhq/cli` |

The independent [`brain`](https://github.com/aexhq/brain) repository owns the session API and
Brain-to-Hand protocol. [`hands`](https://github.com/aexhq/hands) implements Brain's Hand ports.
Aex composes both at immutable source revisions.

## Develop

Requires Rust 1.97, Node.js 22 or later, Python 3, and `cargo-typify`.

```sh
tools/gen.sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
npm ci
npm test
```

Change schemas before generated Rust or TypeScript files, then run `tools/gen.sh`. CI rejects
generated drift.

Apache-2.0 licensed.
