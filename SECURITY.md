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

- The published `@aexhq/sdk` package.
- The published `aex` command-line binaries and their signed archives.
- The published wire contract under `api/` and the crates generated from it.
- The agent execution runtime published from this repository, including the
  builtin agent tools and the boundaries they rely on.
- Hosted Aex control-plane vulnerabilities that affect SDK, CLI, or runtime
  users, even though the hosted implementation source is maintained privately.

Out of scope:

- Vulnerabilities in third-party dependencies — please report those
  upstream and link the advisory.
- Findings against `https://aex.dev` that require workspace
  access you weren't authorised to use.
- DoS via unbounded inputs that the platform already documents as
  caller-controlled (message size, prompt size, output retention).

## Model access, data flow, and secret handling

### Your provider key

You bring the model key. Session creation requires `provider`, an exact
provider-native `model`, and a write-only `providerApiKey`. That key is
encrypted for the one session, is never returned by any endpoint, and is never
reused by another session. There is no managed model key, and no reusable
provider-credential store.

Put the key in `providerApiKey` and nowhere else. A key pasted into a message,
into session metadata, or into a mounted file becomes ordinary session content,
which Aex does not scan or mask.

Your session talks to the provider you named, so your inputs and the model's
responses are handled under that provider's terms rather than Aex's. Review
them before sending regulated data. Public billing behaviour is documented in
the billing section of [`apps/site/content/docs/`](apps/site/content/docs/).

### Session secrets

Secrets are scoped to one session, not to a workspace. The provider key, remote
MCP request headers, and sandbox-process MCP environment values are all
write-only, encrypted, and frozen when the session is created. The session
configuration Aex reads back omits every plaintext secret, and MCP servers come
back as names without their transport credentials.

There is no separate secret store or secret registry. A workspace file is an
opaque value that any session mounting it can read, so it is not a place for a
credential.

Treat every Aex API key as a secret as well. Keep it out of source control,
logs, metadata, exception messages, and support transcripts.

Registered MCP servers are customer-trusted remote systems. Their credentials
remain subject to the remote provider's security, retention, and revocation
policies.

### Networking and bearer grants

Sandbox egress is explicit. `sandbox.network.hands` is either `none`, which
denies public-internet access from tool execution, or `public_internet`, which
permits direct public networking. There is no customer-visible proxy endpoint
or managed-egress virtual-host protocol. Callers remain responsible for the
destinations, requests, and credentials their sessions use.

File and telemetry downloads return short-lived bearer grants. Anyone holding a
grant URL and its required headers can use it until expiry, so do not log, store,
or forward either value. Expiry prevents a new request but cannot recall bytes
already downloaded.

### Agent output

Aex does not inspect, scan, or mask the bytes an agent writes into messages,
tool output, telemetry, or files. If a session prints a secret, that secret is
part of the resulting customer-visible data. Deciding what an agent may read and
emit remains the workspace owner's responsibility.

Public handling guidance is documented in
the authentication, resources, networking, files and limits sections of
[`apps/site/content/docs/`](apps/site/content/docs/).

Security-sensitive hosted implementation details are maintained privately.
