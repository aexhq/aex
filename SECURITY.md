# Security policy

## Reporting a vulnerability

**Do not file public GitHub issues for security bugs.**

Use GitHub's private vulnerability reporting:
[Security → Report a vulnerability](https://github.com/aexhq/aex/security/advisories/new).
We'll acknowledge within a few business days and coordinate
disclosure with you.

If private reporting isn't available to you, contact
[`@weilueluo`](https://github.com/weilueluo) on GitHub and we'll set
up a private channel.

## Scope

In scope:

- The published `@aexhq/sdk` package (SDK + bundled CLI).
- The public `@aexhq/contracts` package.
- Hosted aex control-plane vulnerabilities that affect SDK/CLI users.

Out of scope:

- Vulnerabilities in third-party dependencies — please report those
  upstream and link the advisory.
- Findings against `https://aex.dev` that require workspace
  access you weren't authorised to use.
- DoS via unbounded inputs that the platform already documents as
  caller-controlled (run size, prompt size, output retention).

## Model access and secret custody

### Managed model access (no customer provider keys)

aex routes every model call through the **Vercel AI Gateway** using a single
**platform-owned** gateway credential per plane, held in SSM. Customers supply
**no** provider API key: neither provider selection nor a per-provider key map is
part of the public contract, and a submission carrying either is rejected. You
name a model by its `creator/model` gateway slug and the managed key handles the
upstream provider relationship, so provider-side processing flows under
**Vercel's** agreements with the model providers rather than any customer's own
provider account.

### Customer secret custody

Workspace `environment.secrets` and MCP credentials are sealed **per session**
under a KMS-wrapped run vault key, and are opened only inside that session's
runtime for its lifecycle. aex attempts terminal cleanup/revocation for
aex-controlled references; credentials, sessions, and data held by third parties
you point a session at remain subject to that provider account's policies.
Reports related to secret leakage, persistence beyond a session, or cross-tenant
secret exposure are high-priority.

### What this does and does not cover

aex seals your secrets **in transit and at rest**. aex does **not** inspect,
scan, or mask what the agent writes into its own output: tool output, the event
stream, the session journal, captured files, and archived logs carry the real
bytes the session produced. If a session prints a secret, that value appears in
those surfaces verbatim. Deciding what an agent may read and emit is the
workspace owner's call.

Credential and data-handling behavior is documented in
`packages/sdk/docs/credentials.md`, `packages/sdk/docs/secrets.md`, and
`packages/sdk/docs/limits.md`.
