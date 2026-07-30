# @aexhq/contracts

The strict public v1 wire contracts for aex.

```bash
npm i @aexhq/contracts
```

Most TypeScript users should install `@aexhq/sdk`, which includes these
contracts in its published build. Install this package directly when building
another client or validating a service against the same wire.

The root export contains:

- bootstrap and regional route descriptors;
- strict account, organization, workspace, billing, session, content, and
  telemetry schemas;
- UUIDv7 identifier and workspace API-key codecs;
- the public error and HTTP transport vocabulary.

The generated plane documents are available as:

- `@aexhq/contracts/openapi/bootstrap.json`
- `@aexhq/contracts/openapi/regional.json`

Schemas are the source. Generate and check the committed OpenAPI documents with
`bun run openapi:generate`, `bun run openapi:check`, and their corresponding
`openapi:types:*` commands from the repository root.

## License

Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
