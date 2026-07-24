---
title: Secrets
---

# Secrets

aex supports per-session credentials and reusable workspace secrets for your own
code and MCP servers. Model access needs no provider key — the managed gateway
routes every model. Secret values are excluded from the idempotency fingerprint
and do not belong in session config.

Runnable examples need only `AEX_API_KEY` for aex.

## Run A Model In One Session

### TypeScript

```ts
import { Aex } from "@aexhq/sdk";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });

await aex.start({
  model: "anthropic/claude-haiku-4-5",
  message: "Write a short report and save it as a file.",
});
```

### CLI

```bash
aex start \
  --api-key "$AEX_API_KEY" \
  --model anthropic/claude-haiku-4-5 \
  --prompt "Write a short report and save it as a file."
```

## Persist An Env Secret

Create durable secrets through the workspace namespace, then reference the
stored name in later sessions.

```ts
import { Aex, Secret } from "@aexhq/sdk";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });

await aex.workspace.secrets.set({
  name: "github-token",
  value: process.env.GITHUB_TOKEN!
});
const githubToken = Secret.ref("github-token");

await aex.start({
  model: "anthropic/claude-haiku-4-5",
  message: "Inspect the repository issues.",
  environment: { secrets: { GITHUB_TOKEN: githubToken } },
});
```

## Set Or Rotate A Workspace Secret

Use `client.workspace.secrets.set(...)` to create a named secret directly. Use
`client.workspace.secrets.rotate(...)` to replace its value while keeping the same name.

```ts
await aex.workspace.secrets.set({
  name: "serper-api-key",
  value: process.env.SERPER_API_KEY!
});

await aex.workspace.secrets.rotate({
  name: "serper-api-key",
  value: process.env.SERPER_API_KEY_NEXT!
});
```

## Retrieve Secret Metadata

`list` and `get` return metadata only. They never return the secret value.

```ts
const secrets = await aex.workspace.secrets.list();
const metadata = await aex.workspace.secrets.get("serper-api-key");
```

## Inject A Workspace Secret Into A Session

Reference workspace secrets with `Secret.ref(name)`. The value resolves
server-side and is injected as the named environment variable.

```ts
import { Secret } from "@aexhq/sdk";

await aex.start({
  model: "anthropic/claude-haiku-4-5",
  message: "Use SERPER_API_KEY for web search.",
  environment: {
    secrets: { SERPER_API_KEY: Secret.ref("serper-api-key") }
  },
});
```

## Delete A Workspace Secret

```ts
await aex.workspace.secrets.delete("serper-api-key");
```

The CLI supports per-session runtime and MCP credentials. Workspace secret
administration is exposed through the SDK.

## Redaction Scope And Session Files

Registered secret values are redacted from the session's **event stream** (both tool
output and model-authored surfaces) — a value you inject via `environment.secrets`
is masked regardless of its shape. Two surfaces are intentionally *not* scrubbed:

- **Captured session files** (`session.files.download()` / `.read()` / the `aex download`
  zip) are returned **verbatim**. They are your session's own artifacts, so the platform
  does not rewrite their bytes — if the agent writes a secret into a deliverable file,
  that file contains it. Treat downloaded files as unredacted.
- An **unregistered** secret (a credential the session produces itself and never declared
  via `environment.secrets`) can only be masked heuristically by shape; register the
  values you care about so they are masked by value.
