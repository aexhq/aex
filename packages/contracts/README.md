# @aexhq/contracts

The public wire contracts for aex: request and response schemas, the event
envelope, id formats, and the generated OpenAPI document for the data plane.

```bash
npm i @aexhq/contracts
```

## Who needs this

Most people do not. `@aexhq/sdk` inlines this package into its own build, so an
SDK user already has every type without adding a dependency.

Install it directly when you are:

- writing a client in TypeScript without the SDK;
- generating a client in another language from
  `@aexhq/contracts/openapi/data-plane.json`;
- validating that a server or proxy you operate speaks the same wire format.

## What is in it

| Export | Contents |
| --- | --- |
| `@aexhq/contracts` | Session, run, workspace, and file schemas; error shapes. |
| `@aexhq/contracts/ids` | Typed id constructors and parsers. |
| `@aexhq/contracts/testing` | Fixtures and helpers for conformance tests. |
| `@aexhq/contracts/subagent-runtime` | The parent/child submission contract. |
| `@aexhq/contracts/openapi/data-plane.json` | The generated OpenAPI document. |

Schemas are [Zod](https://zod.dev) and expose the
[Standard Schema](https://standardschema.dev) interface, so they validate under
any Standard Schema-aware validator without importing Zod yourself.

## The generation direction

Schemas are the source. The OpenAPI document and its TypeScript declarations are
generated from them and committed, and CI fails when the committed output no
longer matches (`bun run openapi:check`, `bun run openapi:types:check` at the
repository root). Edit the schemas, never the generated document.

## Stability

The contracts version tracks the wire format, not the SDK. A breaking wire
change is a major version here. `@aexhq/contracts/internal` is exempt: it exists
for first-party runtimes and carries no compatibility promise.

## License

Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
