---
title: Networking
---

# Networking

A session executes your agent's code in a sandbox with **no direct route to the
internet**. Outbound traffic is governed by **two layers**:

1. **The per-session policy (`environment.networking`)** — enforced by the agent
   runtime *inside the session*. It applies to the standard proxy path every normal
   HTTP client uses (see below) and can only *narrow* what the session may reach.
2. **The platform ceiling** — a fixed, platform-managed egress boundary every
   connection ultimately traverses. It allows the hosts aex itself manages
   (model providers, built-in tool endpoints, package registries, and related
   well-known development hosts such as `github.com`) and enforces a fixed SSRF
   deny-list: loopback, link-local, cloud-metadata, and other private ranges are
   blocked on the standard proxy path every normal HTTP client uses, including
   hostnames that resolve to those ranges. Note: on the current managed (Fargate)
   plane a subprocess that deliberately bypasses the proxy with a **raw socket**
   can still reach the on-link task-metadata IP (`169.254.170.2`), which exposes
   non-secret task identity (AWS account id via the task ARN, cluster/image ref)
   — but **never IAM credentials** (the session's task role is unset, so the metadata
   credential endpoint serves nothing) and never another tenant's data.

Honest boundary statement: the per-session `allowedHosts` policy is enforced by the
session's own runtime on the standard proxy path — it is **not** yet enforced at
the platform proxy layer. A subprocess that deliberately bypasses the standard
proxy environment (a raw socket / raw CONNECT) is bounded by the **platform
ceiling** rather than by the per-session list. Per-session enforcement at the platform
proxy layer is planned; until it ships, treat `allowedHosts` as a strong
default-path control and an auditable statement of intent, not a hard isolation
boundary against adversarial code inside the session.

**Default posture.** A session that does not set `environment.networking` sessions in
`open` mode: its own code may reach anything within the platform ceiling with
no allowlist required. Use `environment.networking` to *narrow* that surface
when you want a tighter, auditable egress posture. Code cannot widen the
ceiling from inside the container.

## Paths that always work

These reach the network over managed platform paths and are **not** subject to
`environment.networking`, so you never list their hosts:

- The model / provider call for the session (and its subagents).
- The built-in `web_search` and `web_fetch` tools. They run over a managed,
  SSRF-guarded server-side path, which is why they can reach arbitrary public
  URLs even though your own code is bounded by the ceiling.
- Remote MCP servers you declare in `mcpServers` — MCP traffic rides a managed
  path; see [MCP](mcp.md).
- The package registries for any `environment.packages` you declare (pip → PyPI,
  apt → the distribution mirrors). Declaring a package implicitly allows the
  registry it installs from.

`environment.networking` governs the **other** case: outbound that your own
code makes — a `curl` in the `bash` tool, a `requests`/`urllib` call in Python,
a `fetch` in `code_execution`, or a third-party SDK.

## Restrict a session to an allowlist

Set `mode: "limited"` and list exactly the hosts your code is allowed to reach.
The session's runtime enforces the list on the standard proxy path: a connection to
a host that is neither on the list nor one of the always-allowed paths above is
refused before it leaves the session. Package-registry hosts implied by
`environment.packages` are appended automatically so installs keep working.

Note that `allowedHosts` narrows *within* the platform ceiling — listing a host
does not by itself make it reachable if the platform ceiling does not carry it.
If your run needs a host the ceiling blocks, contact support.

### TypeScript

```ts
import { Aex, Models, Providers } from "@aexhq/sdk";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });

await aex.start({
  provider: Providers.ANTHROPIC,
  model: Models.CLAUDE_HAIKU_4_5,
  message: "Fetch the public status page and summarize it.",
  environment: {
    networking: {
      mode: "limited",
      allowedHosts: ["api.example.com", "status.example.com"]
    }
  },
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});
```

`allowedHosts` entries are host names (lowercased), e.g. `api.example.com`. Add a
non-default port when you need one (`api.example.com:8443`); a bare host name
covers HTTPS on 443. Matching is exact per host — it is not a wildcard or suffix
match, so list each host you need.

Keep the allowlist in your session options so the submitted network policy is
visible at the same call site as the code that needs it.

## Open mode

`open` is the default: a session that omits `environment.networking` already sessions in
open mode. Set `mode: "open"` explicitly when you want to be unambiguous. Open
mode applies no per-session allowlist — the session's own code may reach anything the
platform ceiling allows, still subject to the SSRF deny-list. Prefer `limited`
whenever you can name the hosts — it gives the session a stable, auditable,
least-privilege egress surface (it is the tighter posture, not the default).

```ts
await aex.start({
  model: Models.CLAUDE_HAIKU_4_5,
  message: "Research the topic across the open web.",
  environment: { networking: { mode: "open" } },
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});
```

(Web research like the example above flows through the managed `web_search` /
`web_fetch` path, which is not ceiling-bounded.)

## Transparent for normal HTTP clients

You write ordinary code — there is no per-request proxy configuration and no
client changes. The runtime sets the standard proxy environment
(`HTTP_PROXY` / `HTTPS_PROXY`, with `NO_PROXY` for internal hosts), so any client
that honors it — `curl`, Python `requests` / `urllib`, `pip`, `npm`, Node
`fetch`, and most language SDKs — reaches allowed hosts transparently:

```bash
# In the agent's shell, against a limited run that allows api.example.com:
curl -sS https://api.example.com/v1/status   # works
curl -sS https://other-host.example          # refused (not in allowlist)
```

You also do **not** need to install any certificate. The platform manages the
trust store for the managed egress path automatically, so TLS verification in
your client succeeds without extra setup.

## Limitations and gotchas

- **The per-session policy is enforced by the session's runtime, not at the platform
  proxy.** Clients that honor the standard proxy environment (almost all HTTP
  tooling) are held to the `allowedHosts` list. A subprocess that deliberately
  ignores the proxy environment and opens a raw connection is **not** held to
  the per-session list — it is bounded by the platform ceiling (the aex-managed
  provider/tool/registry host set) and the SSRF deny-list instead. Per-session
  enforcement at the platform proxy layer is planned.
- **A client that hard-bypasses the standard environment may fail to connect.**
  A client that ignores the proxy environment, pins or replaces its certificate
  trust store, or speaks a non-HTTP protocol over a raw socket can hit a wall
  even for a host you allowed. The fix is to let the client use the standard
  proxy and certificate environment the runtime provides (most libraries do by
  default), rather than overriding it.
- **`allowedHosts` only applies in `limited` mode.** It is ignored in `open`
  mode, where the platform ceiling and the SSRF deny-list are the gates.
- **`allowedHosts` cannot exceed the platform ceiling.** It narrows; it never
  widens. A listed host outside the ceiling still fails.

For credentialed HTTP calls, pass the credential as an `environment.secrets`
entry and let your code use its normal HTTP client. For remote tool servers, see
[MCP](mcp.md). For the full set of session-config fields, see
[SessionRecord configuration](session-config.md).
