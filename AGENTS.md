# Working in Aex

- Aex is the official hosted composition of Brain. Consume Brain's neutral contracts and
  runtime from immutable releases; never copy its session engine, journal, SDK transport,
  model adapters, or Environment protocol.
- This repository must be independently buildable and suitable for public release. Private
  deployment configuration, credentials, customer records, and commercial policy belong in
  Platform. There is no build or test dependency on that private repository.
- Read [ROADMAP.md](ROADMAP.md) and [the ADR index](docs/adr/README.md). Proposed decisions
  are discussion drafts, not implemented behavior or accepted requirements.
- Keep one owner for each behavior. Start with cohesive modules in one service; extract a
  package, trait, or process when a concrete dependency or execution boundary requires it.
- Before adding a layer or check, name its supported failure and the action on failure.
  Reuse existing code, the standard library, platform features, and installed dependencies
  before writing new mechanisms. Preserve validation, security, and data-loss handling.
- Authenticate and authorize at customer boundaries; assume cooperation within trusted
  internals. Unknown public operations and unsupported grants fail explicitly.
- Generate Aex-owned contracts from their implementing types and route annotations.
  Do not redefine Brain types. Keep generated output with its source change.
- No skipped tests or bypassed CI gates. CI gates release; local success does not replace it.
- Keep local environment files at the workspace root, never in this repository. Never
  commit or print secret values.
- Keep current behavior in code and user documentation. Research and abandoned approaches
  belong in the private research repository. Comments explain non-obvious reasons only.
- Commit style: `area: imperative summary`.
