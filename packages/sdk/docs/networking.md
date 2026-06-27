---
title: Networking
---

# Networking

A run executes your agent's code in a sandbox that has **no unmediated route to
the internet**. Every outbound connection — whether it comes from the model, a
built-in tool, or a `curl` your code runs in the shell — passes through the
platform's egress boundary, which enforces the run's networking policy and a
fixed SSRF deny-list (loopback, link-local, cloud-metadata, and other private
ranges are always blocked, including hostnames that resolve to those ranges).

**Networking is open by default.** A run that does not set
`environment.networking` may reach any public host — still subject to the SSRF
deny-list above — with no allowlist required. You use the `environment.networking`
field to *narrow* that surface when you want a tighter, auditable egress posture.
Code cannot widen the policy from inside the container: the boundary is the
platform's, not the agent's.

## Paths that always work

These reach the network over managed paths and are **not** subject to
`environment.networking`, so you never list their hosts:

- The model / provider call for the run (and its subagents).
- The built-in `web_search` and `web_fetch` tools (still SSRF-guarded).
- Any remote MCP servers you declare in `mcpServers` — see [MCP](mcp.md).
- Any `proxyEndpoints` you declare — see [Credentials](credentials.md).
- The package registries for any `environment.packages` you declare (pip → PyPI,
  apt → the distribution mirrors). Declaring a package implicitly allows the
  registry it installs from.

`environment.networking` governs the **other** case: arbitrary outbound that
your own code makes to a host the platform doesn't already manage — a `curl` in
the `bash` tool, a `requests`/`urllib` call in Python, a `fetch` in
`code_execution`, or a third-party SDK.

## Restrict a run to an allowlist

Set `mode: "limited"` and list exactly the hosts your code is allowed to reach.
Anything not on the list (and not one of the always-allowed paths above) is
blocked, and the call fails. The platform automatically appends the
infrastructure hosts aex itself needs, so the model and tool paths keep working
without you listing them.

### TypeScript

```ts
import { AgentExecutor, Models, Providers } from "@aexhq/sdk";

const aex = new AgentExecutor({ apiToken: process.env.AEX_API_TOKEN! });

await aex.submit({
  provider: Providers.ANTHROPIC,
  model: Models.CLAUDE_HAIKU_4_5,
  prompt: "Fetch the public status page and summarize it.",
  environment: {
    networking: {
      mode: "limited",
      allowedHosts: ["api.example.com", "status.example.com"]
    }
  },
  secrets: { apiKey: process.env.ANTHROPIC_API_KEY! }
});
```

`allowedHosts` entries are host names (lowercased), e.g. `api.example.com`. Add a
non-default port when you need one (`api.example.com:8443`); a bare host name
covers HTTPS on 443. Matching is exact per host — it is not a wildcard or suffix
match, so list each host you need.

To validate your allowlist before submitting, `buildPlatformAllowedHosts` returns
the host set the platform will enforce given a base URL plus your extra hosts:

```ts
import { buildPlatformAllowedHosts } from "@aexhq/sdk";

const allowedHosts = buildPlatformAllowedHosts({
  baseUrl: "https://api.aex.dev",
  extraHosts: ["api.example.com"]
});
```

## Open mode

`open` is the default: a run that omits `environment.networking` already runs in
open mode. Set `mode: "open"` explicitly when you want to be unambiguous, or when
a run needs to reach hosts you can't enumerate ahead of time. The run may then
reach any public host, still subject to the SSRF deny-list. Prefer `limited`
whenever you can name the hosts — it gives the run a stable, auditable, least-
privilege egress surface (it is the tighter posture, not the default).

```ts
await aex.submit({
  model: Models.CLAUDE_HAIKU_4_5,
  prompt: "Research the topic across the open web.",
  environment: { networking: { mode: "open" } },
  secrets: { apiKey: process.env.ANTHROPIC_API_KEY! }
});
```

## Transparent for normal HTTP clients

You write ordinary code — there is no per-request proxy configuration and no
client changes. The runtime sets the standard proxy environment
(`HTTP_PROXY` / `HTTPS_PROXY`, with `NO_PROXY` for internal hosts), so any client
that honors it — `curl`, Python `requests` / `urllib`, `pip`, `npm`, Node
`fetch`, and most language SDKs — reaches allowed hosts transparently:

```bash
# In the agent's shell, against a limited run that allows api.example.com:
curl -sS https://api.example.com/v1/status   # works
curl -sS https://other-host.example          # blocked (not in allowlist)
```

You also do **not** need to install any certificate. The platform manages the
trust store for the managed egress path automatically, so TLS verification in
your client succeeds without extra setup.

## Limitations and gotchas

- **Enforcement is at the platform boundary, not in your code.** A tool can't
  bypass the policy by ignoring proxy settings or opening a raw socket — the
  sandbox has no other route out, so a disallowed host simply fails. This is the
  intended fail-closed behavior.
- **A client that hard-bypasses the standard environment may fail to connect.**
  This is rare — almost all HTTP tooling honors `HTTP_PROXY` / `HTTPS_PROXY` and
  the system trust store. But a client that is explicitly told to ignore the
  proxy environment, pins or replaces its certificate trust store, or speaks a
  non-HTTP protocol over a raw socket can hit a wall even for an allowed host.
  The fix is to let the client use the standard proxy and certificate
  environment the runtime provides (most libraries do by default), rather than
  overriding it.
- **`allowedHosts` only applies in `limited` mode.** It is ignored in `open`
  mode, where the SSRF deny-list is the only gate.

For routing credentialed HTTP calls through the managed proxy without putting the
secret in the container, use proxy endpoints — see
[Credentials](credentials.md). For remote tool servers, see [MCP](mcp.md). For
the full set of run-config fields, see [Run configuration](run-config.md).
