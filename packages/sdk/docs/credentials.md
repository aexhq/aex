---
title: Credentials
---

# Credentials

aex does not store provider keys or MCP credential values across runs.

The caller passes a workspace-scoped SDK token and the provider key inline on every `submitRun` call. aex holds the bundle in run-scoped custody for the run lifecycle and attempts terminal cleanup/revocation for the aex-controlled references. MCP credentials and proxy endpoint auth values travel the same way.

A run targets exactly one provider (selected by `provider`, default `anthropic`), so the key is a single flat field:

| Field | Required secret |
| --- | --- |
| Provider API key | `secrets.apiKey` |

The same `secrets.apiKey` carries the BYOK key for whichever `provider` the run selects.

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

The platform's managed HTTP proxy is the agent-first alternative. The caller declares **policy** at the top level of the submission (hashed for idempotency) and supplies the matching **auth value** inside `secrets` (not hashed, so key rotation does not collapse onto a stale run). The raw credential value never enters the container.

```ts
import {
  AgentExecutor,
  validateProxyAuth,
  buildPlatformAllowedHosts
} from "@aexhq/sdk";

const aex = new AgentExecutor({
  apiToken: "ant_..."
});

const proxyEndpoints = [
  {
    name: "stripe",
    baseUrl: "https://api.stripe.com",
    authShape: { type: "bearer" },
    allowMethods: ["GET", "POST"],
    allowPathPrefixes: ["/v1/charges", "/v1/refunds"],
    maxRequestBytes: 65_536,
    maxResponseBytes: 65_536,
    timeoutMs: 10_000,
    responseMode: "headers_only",
    perCallBudget: 60,
    responseByteBudget: 1_048_576
  }
] as const;

const proxyEndpointAuth = [
  {
    name: "stripe",
    value: { type: "bearer", token: process.env.STRIPE_API_KEY! }
  }
] as const;

// Fail fast at submission time when policy and auth disagree.
validateProxyAuth(proxyEndpoints, proxyEndpointAuth);

const runId = await aex.submitRun({
  model: "claude-haiku-4-5",
  prompt: "…",
  proxyEndpoints,
  secrets: {
    apiKey: process.env.ANTHROPIC_API_KEY!,
    proxyEndpointAuth
  }
});
```

Inside the run container, every session has the platform CLI mounted at `/mnt/session/uploads/aex/aex` (a Node ESM bundle) and a manifest at `/mnt/session/uploads/aex/index.json` describing the declared endpoints. The skill invokes the CLI through `node` (the mount has no execute permission so direct invocation fails with `bad interpreter: Permission denied`):

```bash
node /mnt/session/uploads/aex/aex proxy stripe \
  --method GET \
  --path /v1/charges/ch_123 \
  --response-mode headers_only
```

The CLI reads the per-run bearer from `/mnt/session/uploads/aex/run-token`, attaches the `X-Aex-Proxy-Protocol` header, and the BFF injects the bearer/header/query/basic credential before dispatching the outbound call. Only the response (subject to `responseMode` and `maxResponseBytes`) reaches the container. `--response-mode` can only narrow below the policy ceiling.

#### Keyless upstreams (`authShape: { type: "none" }`)

For public APIs that take no credential (Wikimedia Commons, Internet Archive, Library of Congress, NASA Images, NARA, GDELT, etc.), declare the endpoint with `authShape: { type: "none" }` and omit the matching `proxyEndpointAuth[]` entry entirely:

```ts
const proxyEndpoints = [
  {
    name: "wikimedia",
    baseUrl: "https://commons.wikimedia.org",
    authShape: { type: "none" },
    allowMethods: ["GET"],
    allowPathPrefixes: ["/wiki/", "/w/api.php"]
  }
] as const;

const runId = await aex.submitRun({
  model: "claude-haiku-4-5",
  prompt: "…",
  proxyEndpoints,
  secrets: { apiKey: process.env.ANTHROPIC_API_KEY! }
});
```

The keyless endpoint still routes through the aex managed proxy: every call is allow-listed, audited, redacted, and counted against per-run budgets. The BFF injects no `Authorization` header and no query-string credential. Shipping a `proxyEndpointAuth` entry for a `none`-shape endpoint is rejected at submission time. Equivalent class-based form:

```ts
import { ProxyEndpoint } from "@aexhq/sdk";

ProxyEndpoint.none({
  name: "wikimedia",
  baseUrl: "https://commons.wikimedia.org",
  allowMethods: ["GET"],
  allowPathPrefixes: ["/wiki/", "/w/api.php"]
});
```

`node /mnt/session/uploads/aex/aex --help` reads endpoint details from `/mnt/session/uploads/aex/index.json`. Runs that do not declare any `proxyEndpoints` still have the CLI and an empty manifest mounted, so agents never need to introspect whether the surface exists.

### Networking

When a run uses `limited` networking, the platform host must appear in `allowed_hosts`. The worker injects it automatically; for advance validation use:

```ts
const allowedHosts = buildPlatformAllowedHosts({
  baseUrl: "https://api.aex.dev",
  extraHosts: ["api.stripe.com"]
});
```

### Secrets are always explicit at the call site

There is no `defaultSecrets` and no client-held secret state. Every `submitRun` call carries its full `secrets` bundle (one provider key + optional MCP credentials + optional `proxyEndpointAuth`). This is the agent-first invariant: the credentials being used on any given call are visible in the same line of code that submits the run.

