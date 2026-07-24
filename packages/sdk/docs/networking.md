---
title: Networking
---

# Networking

Session networking is managed by aex. Your code can make ordinary outbound HTTP
requests, subject to the session's `environment.networking` policy and the
service-wide safety boundary. Private, loopback, link-local, and otherwise
unsafe destinations remain blocked.

`environment.networking` has two modes:

- `open` is the default and does not require a per-session allowlist.
- `limited` restricts supported outbound HTTP clients to the hosts you list in
  `allowedHosts`.

`allowedHosts` is a useful least-privilege control and an auditable statement of
intent. It is not a security boundary against adversarial code executing inside
the session; low-level networking that bypasses supported HTTP client behavior
may fail and remains subject to the service-wide egress policy.

## Paths that always work

These managed capabilities are not subject to `environment.networking`, so you
do not list their hosts:

- The model/provider call for the session and its subagents.
- The built-in `web_search` and `web_fetch` tools.
- Remote MCP servers declared in `mcpServers`; see [MCP](mcp.md).
- Package registries used by packages declared in `environment.packages`.

`environment.networking` governs other outbound requests made by your code,
such as `curl` in the `bash` tool, Python `requests`, Node `fetch`, or a
third-party SDK.

## Restrict a session to an allowlist

Set `mode: "limited"` and list the hosts your code needs. A supported HTTP
client is refused when it requests a host that is neither listed nor covered by
one of the managed capabilities above. Package registries implied by
`environment.packages` are allowed automatically.

### TypeScript

```ts
import { Aex } from "@aexhq/sdk";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });

await aex.start({
  model: "anthropic/claude-haiku-4-5",
  message: "Fetch the public status page and summarize it.",
  environment: {
    networking: {
      mode: "limited",
      allowedHosts: ["api.example.com", "status.example.com"]
    }
  },
});
```

`allowedHosts` entries are lowercased host names such as `api.example.com`.
Include a non-default port when needed (`api.example.com:8443`); a bare host
covers HTTPS on port 443. Matching is exact, not wildcard or suffix based, so
list each required host.

Keep the allowlist in your session options so the network policy stays next to
the code that needs it.

## Open mode

A session that omits `environment.networking` uses `open` mode. Set
`mode: "open"` explicitly when you want that choice visible in configuration.
Open mode removes the per-session allowlist, but the service-wide safety
boundary still applies.

```ts
await aex.start({
  model: "anthropic/claude-haiku-4-5",
  message: "Research the topic across the open web.",
  environment: { networking: { mode: "open" } },
});
```

Built-in web research uses the managed `web_search` and `web_fetch` tools and
does not require an allowlist entry.

## Ordinary HTTP clients work without extra setup

Use normal HTTP clients without configuring a per-request proxy or installing a
custom certificate. For example, in a limited session that allows
`api.example.com`:

```bash
curl -sS https://api.example.com/v1/status   # works
curl -sS https://other-host.example          # refused (not in allowlist)
```

Clients that override standard network or certificate behavior, or use a
non-HTTP protocol, may fail even for a listed host. Let the client use its
normal defaults whenever possible.

## Limitations

- `allowedHosts` applies only in `limited` mode.
- A listed host can still be unavailable under the service-wide safety policy.
- Exact host matching means subdomains must be listed separately.
- Contact support when a required public host is unavailable.

For credentialed HTTP calls, pass the credential through
`environment.secrets` and let your code use its normal HTTP client. For remote
tool servers, see [MCP](mcp.md). For all session configuration fields, see
[Session configuration](session-config.md).
