# Working in Aex

- Public docs serve newcomers: purpose, benefit, then usage. Follow the shared
  [documentation design and wording guide](https://github.com/aexhq/brain/blob/main/references/documentation.md).
  Lead with hosted setup and runnable examples; link to Brain for shared concepts and contracts.

- Aex is the official hosted composition of Brain. Consume Brain's neutral contracts and
  runtime from immutable releases; never copy its session engine, journal, SDK transport,
  model adapters, or Environment protocol.
- Aex owns prompt-based typed responses, answer validation and corrective turns. Compose
  Brain clients and session handles; SDK convenience does not move product policy upstream.
- This repository must be independently buildable and suitable for public release. Private
  deployment configuration, credentials, customer records, and commercial policy belong in
  Platform. There is no build or test dependency on that private repository.
- Read [ROADMAP.md](ROADMAP.md) and [the ADR index](docs/adr/README.md). Accepted decisions
  define scope; the roadmap separates implementation from outstanding release evidence.
- Keep one owner for each behavior. Start with cohesive modules in one service; extract a
  package, trait, or process when a concrete dependency or execution boundary requires it.
- Before adding a layer or check, name its supported failure and the action on failure.
  Reuse existing code, the standard library, platform features, and installed dependencies
  before writing new mechanisms. Preserve validation, security, and data-loss handling.
- Authenticate and authorize at customer boundaries; assume cooperation within trusted
  internals. Unknown public operations and unsupported grants fail explicitly.
- Generate Aex-owned contracts from their implementing types and route annotations.
  Do not redefine Brain types. Keep generated output with its source change.
- Do not skip tests in selected jobs or bypass CI gates. CI gates release; local success does not replace it.
- Run checks for affected components on pull requests; prose-only README/docs edits do not
  need runtime tests or builds. CI selects jobs from the full PR diff and runs all checks on main.
- Keep local environment files at the workspace root, never in this repository. Never
  commit or print secret values.
- Keep current behavior in code and user documentation. Research and abandoned approaches
  belong in the private research repository. Comments explain non-obvious reasons only.
- Commit style: `area: imperative summary`.
