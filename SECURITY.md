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

### Managed model access

Customers supply no model-provider key. A session names a model with its
`creator/model` slug, and the hosted service brokers the model request. Do not
place a provider credential in a session request, registered resource, message,
metadata, or file.

Model inputs and responses leave the workspace region when the selected managed
model is served. Review the service's current subprocessor and data-processing
terms before sending regulated data. Public billing behavior is documented in
the billing section of [`apps/site/content/docs/`](apps/site/content/docs/).

### Workspace keys and secrets

Treat every aex API key as a secret. Keep it out of source control, logs,
metadata, exception messages, and support transcripts. Workspace secrets have a
metadata-only registry: their values are accepted on write and are not returned
after creation. Sessions select secrets by registered name.

Registered MCP servers are customer-trusted remote systems. Their credentials
remain subject to the remote provider's security, retention, and revocation
policies.

### Networking and bearer grants

Raw Hands networking is explicit. `none` denies public-internet access from
tool execution; `public_internet` permits direct public networking. There is no
customer-visible proxy endpoint or managed-egress virtual-host protocol.
Callers remain responsible for the destinations, requests, and credentials
their sessions use.

File and telemetry downloads return short-lived bearer grants. Anyone holding a
grant URL and its required headers can use it until expiry, so do not log, store,
or forward either value. Expiry prevents a new request but cannot recall bytes
already downloaded.

### Agent output

aex does not inspect, scan, or mask the bytes an agent writes into messages,
tool output, telemetry, or files. If a session prints a secret, that secret is
part of the resulting customer-visible data. Deciding what an agent may read and
emit remains the workspace owner's responsibility.

Public handling guidance is documented in
the authentication, resources, networking, files and limits sections of
[`apps/site/content/docs/`](apps/site/content/docs/).

Security-sensitive hosted implementation details are maintained privately.
