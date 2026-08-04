# `@aexhq/wire`

Generated TypeScript models, strict runtime validators, and identifier helpers
for the AEX v1 wire contract. The source under `src/generated/` is emitted by
`aex-contract-gen` from the same authored schema tree that emits the Rust
`aex-wire` crate; it is not an independently maintained contract.

```ts
import { SessionSchema, type Session } from "@aexhq/wire";

const session: Session = SessionSchema.parse(responseBody);
```

Run `cargo run -p aex-contract-gen -- build` after changing `api/schemas/`.
