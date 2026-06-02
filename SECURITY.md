# Security policy

## Reporting a vulnerability

**Do not file public GitHub issues for security bugs.**

Use GitHub's private vulnerability reporting:
[Security → Report a vulnerability](https://github.com/weilueluo/antpath/security/advisories/new).
We'll acknowledge within a few business days and coordinate
disclosure with you.

If private reporting isn't available to you, contact
[`@weilueluo`](https://github.com/weilueluo) on GitHub and we'll set
up a private channel.

## Scope

In scope:

- The published `antpath` npm package (SDK + bundled CLI).
- Public `@antpath/contracts` and `@antpath/conformance` packages.
- Hosted antpath control-plane vulnerabilities that affect public SDK/CLI
  users, even though hosted service source is documented separately.

Out of scope:

- Vulnerabilities in third-party dependencies — please report those
  upstream and link the advisory.
- Findings against `https://www.antpath.ai` that require workspace
  access you weren't authorised to use.
- DoS via unbounded inputs that the platform already documents as
  caller-controlled (run size, prompt size, output retention).

## BYOK and secret handling

`antpath` is bring-your-own-key. Provider keys and MCP credentials
travel inline with each run, are held in Vault for the lifetime of
that run, and are destroyed at cleanup. Reports related to secret
leakage, persistence beyond a run, or cross-tenant secret exposure
are highest-priority.

Security-sensitive implementation design is documented separately. Public
credential handling behavior is documented in `packages/sdk/docs/credentials.md`
and `packages/sdk/docs/product-boundaries.md`.
