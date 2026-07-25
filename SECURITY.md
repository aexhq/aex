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

## BYOK and secret handling

`aex` is bring-your-own-key. Provider keys and MCP credentials
travel inline with each session and are held in session-scoped custody for the
session lifecycle. aex attempts terminal cleanup/revocation for
aex-controlled references; provider-side credentials, sessions, and data
remain subject to the selected provider account's policies. Reports related to
secret leakage, persistence beyond a run, or cross-tenant secret exposure are
high-priority.

Credential handling behavior is documented in `packages/sdk/docs/secrets.md`,
`packages/sdk/docs/credentials.md`, and `packages/sdk/docs/limits.md`.
