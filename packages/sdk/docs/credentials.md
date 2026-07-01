---
title: Credentials
---

# Credentials

aex treats provider keys, MCP credentials, and proxy endpoint auth as per-run
credentials. Reusable env secrets are documented separately in
[Secrets](secrets.md).

The caller passes a workspace-scoped SDK token and the provider key inline on every `openSession` / `run` call. aex holds the bundle in run-scoped custody for the session lifecycle and attempts terminal cleanup/revocation for the aex-controlled references. MCP credentials and proxy endpoint auth values travel the same way.

A session selects one upstream `provider` (default `anthropic`) and must carry a BYOK
key for it. Keys are supplied per-provider so a session can also hold keys for the
**other** providers its subagents may use:

| Field | Required secret |
| --- | --- |
| Provider API keys | `apiKeys` (top-level, keyed by provider) |

```ts
// The session's own provider key, plus extra keys its subagents can use.
apiKeys: {
  anthropic: process.env.ANTHROPIC_API_KEY!, // the session's provider
  deepseek: process.env.DEEPSEEK_API_KEY!     // for a cross-provider subagent
}
```

A `subagent` spawned with a different-family model **inherits the parent's keys
server-side** from the session's vaulted bundle — the keys never transit the
container. If the parent holds no key for the child's provider, the child is
rejected with `parent_missing_provider_key`.

MCP credential types:

- `static_bearer`;
- `oauth_access_token`.

Unsupported:

- arbitrary headers;
- OAuth refresh;
- persisted aex vault.

For managed-runtime runs, aex injects the matching BYOK provider key at the hosted provider-proxy. Provider-side sessions and data remain subject to the selected provider account's retention and deletion policies.

## Proxy endpoints (per-run custom HTTP credentials)

Some skills need to call non-MCP HTTP services (e.g. Stripe, internal APIs). Embedding the credential in the skill content puts the raw secret on disk in the agent container and in the model's context — both prompt-injection-readable.

The platform's managed HTTP proxy is the agent-first alternative. Declare each endpoint with a `ProxyEndpoint.*` constructor: the instance carries the non-secret **policy** (hashed for idempotency) and its **auth token** together at the call site. The SDK splits the token into the vaulted secrets channel server-side (not hashed, so key rotation does not collapse onto a stale run), and the raw credential value never enters the container.

```ts
import { Aex, Models, ProxyEndpoint } from "@aexhq/sdk";

const aex = new Aex({
  apiToken: "ant_..."
});

const stripe = ProxyEndpoint.bearer({
  name: "stripe",
  baseUrl: "https://api.stripe.com",
  token: process.env.STRIPE_API_KEY!,
  allowMethods: ["GET", "POST"],
  allowPathPrefixes: ["/v1/charges", "/v1/refunds"],
  maxRequestBytes: 65_536,
  maxResponseBytes: 65_536,
  timeoutMs: 10_000,
  responseMode: "headers_only",
  retry: {
    maxAttempts: 3,
    initialDelayMs: 250,
    maxDelayMs: 5000,
    jitter: "full",
    retryOnStatuses: [408, 425, 429, 500, 502, 503, 504],
    retryOnMethods: ["GET", "HEAD"],
    respectRetryAfter: true
  }
});

await aex.run({
  model: Models.CLAUDE_HAIKU_4_5,
  message: "…",
  proxyEndpoints: [stripe],
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});
```

The five constructors — `ProxyEndpoint.none` / `bearer` / `header` / `basic` / `query` — put the auth secret on the same call as the policy, so any drift (wrong `responseMode`, misnamed auth field) is a TypeScript error at the call site instead of an HTTP 400 a round-trip later.

Inside the run container, every session has the platform CLI mounted at `/mnt/session/uploads/aex/aex` (a Bun-compatible ESM bundle) and a manifest at `/mnt/session/uploads/aex/index.json` describing the declared endpoints. The skill invokes the CLI through `bun` (the mount has no execute permission so direct invocation fails with `bad interpreter: Permission denied`):

```bash
bun /mnt/session/uploads/aex/aex proxy stripe \
  --method GET \
  --path /v1/charges/ch_123 \
  --response-mode headers_only
```

The CLI reads the per-run bearer from `/mnt/session/uploads/aex/run-token`, attaches the `X-Aex-Proxy-Protocol` header, and the hosted proxy injects the bearer/header/query/basic credential before dispatching the outbound call. Only the response (subject to `responseMode` and `maxResponseBytes`) reaches the container. `--response-mode` can only narrow below the policy ceiling.

Retries are declaration-based. Add `retry` to the endpoint policy when safe for that upstream; runs without `retry` keep single-attempt behavior. `maxAttempts` counts the initial request, and defaults apply only when `retry` is present: `maxAttempts: 3`, `initialDelayMs: 250`, `maxDelayMs: 5000`, `jitter: "full"`, `retryOnStatuses: [408, 425, 429, 500, 502, 503, 504]`, `retryOnMethods: ["GET", "HEAD"]`, and `respectRetryAfter: true`. There are no per-call `aex proxy` retry flags.

#### Keyless upstreams (`authShape: { type: "none" }`)

For public APIs that take no credential (Wikimedia Commons, Internet Archive, Library of Congress, NASA Images, NARA, GDELT, etc.), declare the endpoint with `ProxyEndpoint.none(...)` — it produces only a declaration, no auth token:

```ts
import { ProxyEndpoint } from "@aexhq/sdk";

const wikimedia = ProxyEndpoint.none({
  name: "wikimedia",
  baseUrl: "https://commons.wikimedia.org",
  allowMethods: ["GET"],
  allowPathPrefixes: ["/wiki/", "/w/api.php"]
});

await aex.run({
  model: Models.CLAUDE_HAIKU_4_5,
  message: "…",
  proxyEndpoints: [wikimedia],
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});
```

The keyless endpoint still routes through the aex managed proxy: every call is allow-listed, audited, and redacted. The hosted proxy injects no `Authorization` header and no query-string credential.

`bun /mnt/session/uploads/aex/aex --help` reads endpoint details from `/mnt/session/uploads/aex/index.json`. Runs that do not declare any `proxyEndpoints` still have the CLI and an empty manifest mounted, so agents never need to introspect whether the surface exists.

### Networking

Networking is open by default. When a run explicitly uses `limited` networking,
the platform host must appear in `allowed_hosts`. aex injects it
automatically; for advance validation use:

```ts
const allowedHosts = buildPlatformAllowedHosts({
  baseUrl: "https://api.aex.dev",
  extraHosts: ["api.stripe.com"]
});
```

### Secrets are always explicit at the call site

There is no `defaultSecrets` and no client-held secret state. Every `openSession` / `run` call carries its own credentials at the call site: the top-level `apiKeys` map (one provider key, plus any subagent keys), MCP auth on each `McpServer` instance, and proxy auth on each `ProxyEndpoint` instance. This is the agent-first invariant: the credentials being used on any given call are visible in the same code that opens the session.
