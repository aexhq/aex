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
- The published `@aexhq/cli` and `@aexhq/contracts` packages.
- The agent execution runtime published from this repository, including the
  builtin agent tools and the boundaries they rely on.
- Hosted aex control-plane vulnerabilities that affect SDK, CLI, or runtime
  users, even though the hosted implementation source is maintained privately.

Out of scope:

- Vulnerabilities in third-party dependencies — please report those
  upstream and link the advisory.
- Findings against `https://aex.dev` that require workspace
  access you weren't authorised to use.
- DoS via unbounded inputs that the platform already documents as
  caller-controlled (run size, prompt size, output retention).

## Model access, data flow, and secret handling

### Managed model access (no customer provider keys)

aex routes every model call through the **Vercel AI Gateway** using a single
platform-owned gateway key held per plane in SSM. Customers supply **no**
provider API key: neither provider selection nor a per-provider key map is part
of the public contract, and a submission carrying either is rejected. You name a
model by its `creator/model` gateway slug and the managed key handles the
upstream provider relationship.

### Vercel AI Gateway subprocessor and prompt transit

Prompts, model inputs, and responses transit the Vercel AI Gateway in-transit as
part of serving each model call; that hop is authenticated with aex's single
platform-owned gateway key. The gateway operates zero-data-retention by default:
per Vercel's documented posture it deletes prompt and response content after each
request and retains only request metadata (token counts, model, timing) for
billing and operational accounting. Because access is managed, provider-side
processing flows under **Vercel's** agreements with the upstream model providers
rather than under any customer's own provider account.

There is **no contractual EU regional pinning for the gateway hop**: the gateway
routes to the nearest available region for the selected model. Journal and
telemetry data residency (eu-west-1) is unchanged; only the model-serving hop is
not region-pinned.

LLM tokens are a billed usage dimension: aex pays the gateway per token and
meters that cost to the workspace. Public billing behavior is documented in
[`packages/sdk/docs/billing.md`](packages/sdk/docs/billing.md).

### Non-LLM secrets

The gateway key is the only model credential and it is **platform-owned**;
customer material never carries it. Customer workspace `environment.secrets` and
MCP credentials are sealed **per session** under a KMS-wrapped run vault key and
are opened only inside that session's runtime, for its lifecycle. aex attempts
terminal cleanup and revocation for aex-controlled references; credentials,
sessions, and data held by third parties a session is pointed at remain subject
to that provider account's policies. Reports related to secret leakage,
persistence beyond a session, or cross-tenant secret exposure are high-priority.

### What this does and does not cover

aex seals customer secrets **in transit and at rest**. aex does **not** inspect,
scan, or mask what the agent writes into its own output: tool output, the event
stream, the session journal, captured files, and archived logs carry the real
bytes the session produced. A session that prints a secret surfaces that value
verbatim. Deciding what an agent may read and emit is the workspace owner's call.

Public credential and data-handling behavior is documented in
[`packages/sdk/docs/credentials.md`](packages/sdk/docs/credentials.md),
[`packages/sdk/docs/secrets.md`](packages/sdk/docs/secrets.md), and
[`packages/sdk/docs/limits.md`](packages/sdk/docs/limits.md).

## Reading the published source

Published runtime code names several hosts that do not resolve on the public
internet — `egress.internal`, `web.internal`, `llm.internal`, `journal.internal`,
`events.internal`, `aex.internal`. They are virtual hosts terminated by the
managed egress boundary, not reachable endpoints, and they are documented in
[`references/internal-protocol.md`](references/internal-protocol.md). Read that
page before filing a report about them: the security property is that the
container addressing those hosts is **untrusted**, and every credential is
injected on the far side of the boundary.

Security-sensitive hosted implementation design is maintained privately.
