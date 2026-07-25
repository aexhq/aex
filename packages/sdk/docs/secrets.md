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

## What Happens To A Secret Value In Your Session

aex does **not** scan, mask, or drop your session's content. A registered secret value
that the agent prints, writes, or echoes appears **verbatim** in every surface you can
read:

- the **event stream** (tool output and model-authored text),
- the **session journal** and `session.events` / the event archive,
- **captured session files** (`session.files.download()` / `.read()` / the `aex download`
  zip),
- the container's own stdout/stderr archive.

All four are byte-identical, so nothing you read is a rewritten version of something
else. That is deliberate: your session's data is yours, and a platform that silently
rewrote your bytes would give you an artifact you cannot trust and a value we might have
corrupted (a "secret-shaped" build hash or file path is indistinguishable from a
credential to any scanner).

What registering a secret via `environment.secrets` **does** give you:

- the value is stored encrypted and injected into the session env at run time — it is
  never part of your submitted request body, and it is not written to your session
  config;
- it is scoped to the session, and only the session's own process can fetch it;
- it never reaches a customer-controlled subprocess it was not declared for.

If a value must not appear in a transcript you keep or share, do not let the agent print
it: prefer a tool that consumes the credential internally over one that echoes it, and
review a session's output before forwarding it.
