---
title: Credentials
---

# Credentials

aex uses explicit, per-session credentials:

- `AEX_API_KEY` authenticates the SDK or CLI to aex.
- `apiKeys` carries BYOK provider keys for the model provider.
- `McpServer.remote(..., { headers })` carries MCP auth when a remote MCP server needs it.
- `environment.secrets` carries runtime secrets for your own code.

Secrets never belong in reusable run config, files, prompts, or examples.

## The client credential

Pass your aex API key directly to the constructor — `new Aex(apiKey)` — or as
the `apiKey` option:

```ts
import { Aex } from "@aexhq/sdk";

const aex = new Aex(process.env.AEX_API_KEY!);          // preferred shorthand
// equivalently:
// const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
```

See [Authentication](authentication.md) for how keys are scoped, rotated, and
issued during the beta.

## Provider keys

A session selects one upstream provider and must carry a BYOK key for it. Include
additional provider keys only when subagents may use those providers.

```ts
const result = await aex.run({
  model: Models.CLAUDE_HAIKU_4_5,
  message: "Write a short report and save it as a file.",
  apiKeys: {
    anthropic: process.env.ANTHROPIC_API_KEY!
  }
});
```

Provider keys are used by the managed runtime for model calls. They are not saved
as client defaults.

## Runtime secrets

Use `environment.secrets` for credentials your code needs at runtime. The value
can be ephemeral with `Secret.value(...)` or a workspace secret reference with
`Secret.ref(...)`.

```ts
import { Aex, Models, Secret } from "@aexhq/sdk";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });

await aex.run({
  model: Models.CLAUDE_HAIKU_4_5,
  message: "Call https://api.example.com/v1/status with INTERNAL_API_TOKEN and summarize it.",
  environment: {
    secrets: {
      INTERNAL_API_TOKEN: Secret.value(process.env.INTERNAL_API_TOKEN!)
    },
    networking: {
      mode: "limited",
      allowedHosts: ["api.example.com"]
    }
  },
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});
```

Inside the run, use normal HTTP code for the service:

```bash
curl -sS \
  -H "Authorization: Bearer $INTERNAL_API_TOKEN" \
  https://api.example.com/v1/status
```

## Workspace secrets

> **Availability note:** workspace-secret `Secret.ref(...)` injection requires
> the next platform deploy — on the current hosted plane the referenced
> variable can resolve empty inside the run. Per-run `Secret.value(...)`
> secrets are unaffected.

Store reusable values once, then reference them by name:

```ts
await aex.secrets.set({
  name: "internal-api-token",
  value: process.env.INTERNAL_API_TOKEN!
});

await aex.run({
  model: Models.CLAUDE_HAIKU_4_5,
  message: "Use INTERNAL_API_TOKEN for the status request.",
  environment: {
    secrets: {
      INTERNAL_API_TOKEN: Secret.ref("internal-api-token")
    }
  },
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});
```

Secret reads return metadata only; they never return the stored value.

## Networking

Networking is open by default within the platform's managed egress ceiling. Use
`environment.networking.mode: "limited"` with `allowedHosts` when you want a
run's own code to reach only named hosts. See [Networking](networking.md) for
the two-layer enforcement model.

## Explicit call-site rule

There is no `defaultSecrets` and no client-held secret state. Each
`openSession(...)` or `run(...)` call should show the provider keys, MCP auth, and
runtime secrets needed for that call.
