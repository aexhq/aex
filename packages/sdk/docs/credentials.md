---
title: Credentials
---

# Credentials

antpath does not store provider keys or MCP credential values across runs.

The caller passes a workspace-scoped SDK token and exactly one matching provider key inline on every `submitRun` call. antpath holds the bundle in run-scoped custody for the run lifecycle and attempts terminal cleanup/revocation for the antpath-controlled references. MCP credentials and proxy endpoint auth values travel the same way.

Provider keys are coupled to the submitted `provider`:

| `provider` | Required secret |
| --- | --- |
| `anthropic` | `secrets.anthropic.apiKey` |
| `deepseek` | `secrets.deepseek.apiKey` |
| `openai` | `secrets.openai.apiKey` |
| `gemini` | `secrets.gemini.apiKey` |
| `mistral` | `secrets.mistral.apiKey` |

Supplying a key for any other provider is rejected at submission time.

MCP credential types:

- `static_bearer`;
- `oauth_access_token`.

Unsupported:

- arbitrary headers;
- OAuth refresh;
- persisted antpath vault.

For native provider runtimes, antpath creates per-run provider vault credentials and tracks provider IDs for cleanup attempts. For Goose Managed runs, antpath injects the matching BYOK provider key at the hosted provider-proxy. Provider-side sessions and data remain subject to the selected provider account's retention and deletion policies.

## Proxy endpoints (per-run custom HTTP credentials)

Some skills need to call non-MCP HTTP services (e.g. Stripe, internal APIs). Embedding the credential in the skill content puts the raw secret on disk in the agent container and in the model's context — both prompt-injection-readable.

The platform's managed HTTP proxy is the agent-first alternative. The caller declares **policy** at the top level of the submission (hashed for idempotency) and supplies the matching **auth value** inside `secrets` (not hashed, so key rotation does not collapse onto a stale run). The raw credential value never enters the container.

```ts
import {
  AntpathClient,
  validateProxyAuth,
  buildPlatformAllowedHosts
} from "antpath";

const client = new AntpathClient({
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

const runId = await client.submitRun({
  model: "claude-haiku-4-5",
  prompt: "…",
  proxyEndpoints,
  secrets: {
    anthropic: { apiKey: process.env.ANTHROPIC_API_KEY! },
    proxyEndpointAuth
  }
});
```

Inside the run container, every session has the platform CLI mounted at `/mnt/session/uploads/antpath/antpath` (a Node ESM bundle) and a manifest at `/mnt/session/uploads/antpath/index.json` describing the declared endpoints. The skill invokes the CLI through `node` (the mount has no execute permission so direct invocation fails with `bad interpreter: Permission denied`):

```bash
node /mnt/session/uploads/antpath/antpath proxy stripe \
  --method GET \
  --path /v1/charges/ch_123 \
  --response-mode headers_only
```

The CLI reads the per-run bearer from `/mnt/session/uploads/antpath/run-token`, attaches the `X-Antpath-Proxy-Protocol` header, and the BFF injects the bearer/header/query/basic credential before dispatching the outbound call. Only the response (subject to `responseMode` and `maxResponseBytes`) reaches the container. `--response-mode` can only narrow below the policy ceiling.

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

const runId = await client.submitRun({
  model: "claude-haiku-4-5",
  prompt: "…",
  proxyEndpoints,
  secrets: { anthropic: { apiKey: process.env.ANTHROPIC_API_KEY! } }
});
```

The keyless endpoint still routes through the antpath managed proxy: every call is allow-listed, audited, redacted, and counted against per-run budgets. The BFF injects no `Authorization` header and no query-string credential. Shipping a `proxyEndpointAuth` entry for a `none`-shape endpoint is rejected at submission time. Equivalent class-based form:

```ts
import { ProxyEndpoint } from "antpath";

ProxyEndpoint.none({
  name: "wikimedia",
  baseUrl: "https://commons.wikimedia.org",
  allowMethods: ["GET"],
  allowPathPrefixes: ["/wiki/", "/w/api.php"]
});
```

`node /mnt/session/uploads/antpath/antpath --help` reads endpoint details from `/mnt/session/uploads/antpath/index.json`. Runs that do not declare any `proxyEndpoints` still have the CLI and an empty manifest mounted, so agents never need to introspect whether the surface exists.

### Networking

When a run uses `limited` networking, the platform host must appear in `allowed_hosts`. The worker injects it automatically; for advance validation use:

```ts
const allowedHosts = buildPlatformAllowedHosts({
  baseUrl: "https://api.antpath.ai",
  extraHosts: ["api.stripe.com"]
});
```

### Secrets are always explicit at the call site

There is no `defaultSecrets` and no client-held secret state. Every `submitRun` call carries its full `secrets` bundle (one provider key + optional MCP credentials + optional `proxyEndpointAuth`). This is the agent-first invariant: the credentials being used on any given call are visible in the same line of code that submits the run.

