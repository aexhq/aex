---
title: Secrets
---

# Secrets

aex supports BYOK provider keys, per-run credentials, and reusable workspace
secrets. Secret values are excluded from the idempotency fingerprint and do not
belong in run config.

Runnable examples need both `AEX_API_KEY` for aex and the matching BYOK
provider key, such as `ANTHROPIC_API_KEY` for Claude.

## Use A Provider Key For One Run

### TypeScript

```ts
import { Aex, Models } from "@aexhq/sdk";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });

await aex.run({
  model: Models.CLAUDE_HAIKU_4_5,
  message: "Write a short report and save it as a file.",
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});
```

### CLI

```bash
aex run \
  --api-key "$AEX_API_KEY" \
  --anthropic-api-key "$ANTHROPIC_API_KEY" \
  --model claude-haiku-4-5 \
  --prompt "Write a short report and save it as a file."
```

## Upload An Env Secret

Use `Secret.value(...).upload(...)` when you start with an ephemeral value and
want to persist it as a named workspace secret for later runs.

```ts
import { Aex, Models, Providers, Secret } from "@aexhq/sdk";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });

const githubToken = await Secret.value(process.env.GITHUB_TOKEN!).upload(aex, {
  name: "github-token"
});

await aex.run({
  provider: Providers.ANTHROPIC,
  model: Models.CLAUDE_HAIKU_4_5,
  message: "Inspect the repository issues.",
  environment: { secrets: { GITHUB_TOKEN: githubToken } },
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});
```

## Set Or Rotate A Workspace Secret

Use `client.secrets.set(...)` to create a named secret directly. Use
`client.secrets.rotate(...)` to replace its value while keeping the same name.

```ts
await aex.secrets.set({
  name: "serper-api-key",
  value: process.env.SERPER_API_KEY!
});

await aex.secrets.rotate({
  name: "serper-api-key",
  value: process.env.SERPER_API_KEY_NEXT!
});
```

## Retrieve Secret Metadata

`list` and `get` return metadata only. They never return the secret value.

```ts
const secrets = await aex.secrets.list();
const metadata = await aex.secrets.get("serper-api-key");
```

## Inject A Workspace Secret Into A Run

> **Availability note:** workspace-secret `Secret.ref(...)` injection requires
> the next platform deploy — on the current hosted plane the referenced
> variable can resolve empty inside the run. Per-run `Secret.value(...)`
> secrets are unaffected. This note will be removed once the deploy lands.

Reference workspace secrets with `Secret.ref(name)`. The value resolves
server-side and is injected as the named environment variable.

```ts
import { Models, Providers, Secret } from "@aexhq/sdk";

await aex.run({
  provider: Providers.ANTHROPIC,
  model: Models.CLAUDE_HAIKU_4_5,
  message: "Use SERPER_API_KEY for web search.",
  environment: {
    secrets: { SERPER_API_KEY: Secret.ref("serper-api-key") }
  },
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});
```

## Delete A Workspace Secret

```ts
await aex.secrets.delete("serper-api-key");
```

The CLI supports per-run provider and MCP credentials. Workspace secret
administration is exposed through the SDK.

## Redaction Scope And Output Files

Registered secret values are redacted from the run's **event stream** (both tool
output and model-authored surfaces) — a value you inject via `environment.secrets`
is masked regardless of its shape. Two surfaces are intentionally *not* scrubbed:

- **Captured output files** (`outputs().download()` / `read()` / the `aex download`
  zip) are returned **verbatim**. They are your run's own artifacts, so the platform
  does not rewrite their bytes — if the agent writes a secret into a deliverable file,
  that file contains it. Treat downloaded outputs as unredacted.
- An **unregistered** secret (a credential the run produces itself and never declared
  via `environment.secrets`) can only be masked heuristically by shape; register the
  values you care about so they are masked by value.
