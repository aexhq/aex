---
title: Credentials
---

# Credentials

aex uses explicit, per-session credentials:

- `AEX_API_KEY` authenticates the SDK or CLI to aex.
- `McpServer.remote(..., { headers })` carries MCP auth when a remote MCP server needs it.
- `environment.secrets` carries runtime secrets for your own code.

Model access needs **no** provider API key: aex routes every model through the
managed Vercel AI Gateway with its own key. You name a model by its
`creator/model` gateway slug and nothing else.

Secrets never belong in reusable session config, files, prompts, or examples.

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

## Choosing a model

Name the model by its Vercel AI Gateway `creator/model` slug. There is no
`provider` field and no provider key — the platform's managed gateway key routes
the call.

```ts
const result = await aex.start({
  model: "anthropic/claude-haiku-4-5",
  message: "Write a short report and save it as a file.",
});
```

## Runtime secrets

Use `environment.secrets` for credentials your code needs at runtime. The value
can be ephemeral with `Secret.value(...)` or a workspace secret reference with
`Secret.ref(...)`.

```ts
import { Aex, Secret } from "@aexhq/sdk";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });

await aex.start({
  model: "anthropic/claude-haiku-4-5",
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
});
```

Inside the session, use normal HTTP code for the service:

```bash
curl -sS \
  -H "Authorization: Bearer $INTERNAL_API_TOKEN" \
  https://api.example.com/v1/status
```

## Workspace secrets

Store reusable values once, then reference them by name:

```ts
await aex.workspace.secrets.set({
  name: "internal-api-token",
  value: process.env.INTERNAL_API_TOKEN!
});

await aex.start({
  model: "anthropic/claude-haiku-4-5",
  message: "Use INTERNAL_API_TOKEN for the status request.",
  environment: {
    secrets: {
      INTERNAL_API_TOKEN: Secret.ref("internal-api-token")
    }
  },
});
```

Secret reads return metadata only; they never return the stored value.

## Networking

Networking is open by default within the platform's managed egress ceiling. Use
`environment.networking.mode: "limited"` with `allowedHosts` when you want a
session's own code to reach only named hosts. See [Networking](networking.md) for
the two-layer enforcement model.

## Explicit call-site rule

There is no `defaultSecrets` and no client-held secret state. Each
`aex.sessions.create(...)` or `aex.start(...)` call should show the MCP auth and
runtime secrets needed for that call.
