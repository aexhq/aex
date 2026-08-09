# `@aexhq/wire`

Generated TypeScript models, strict runtime validators, and identifier helpers
for the AEX v1 wire contract. The source under `src/generated/` is emitted by
`aex-contract-gen` from the same authored schema tree that emits the Rust
`aex-wire` crate; it is not an independently maintained contract.

```ts
import { SessionSchema, type Session } from "@aexhq/wire";

const session: Session = SessionSchema.parse(responseBody);
```

This package is `private`: it is a workspace-internal package and is published to
no registry. `@aexhq/sdk` does not import it — the generator emits the SDK's own
route table and error vocabulary into `packages/sdk/src/generated/` so that the
published SDK stays dependency-free.

Run `cargo run -p aex-contract-gen -- build` after changing `api/schemas/`.
