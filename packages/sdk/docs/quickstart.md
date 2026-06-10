---
title: aex quickstart
---

# Quickstart

1. Get an aex SDK API token (`ant_…`).
2. Create `AgentExecutor` — the workspace is derived server-side from the token.
3. Submit the run with the agent's brief plus an inline `secrets` bundle. Wait for terminal status. Fetch outputs.

```ts
import { AgentExecutor, RunModels } from "@aexhq/sdk";

const aex = new AgentExecutor({
  apiToken: process.env.AEX_API_TOKEN!,
  // baseUrl defaults to https://api.aex.dev - set it for local or staging planes.
});

const runId = await aex.submitRun({
  model: RunModels.CLAUDE_HAIKU_4_5,
  prompt: "Write a short answer about agent-first SDK design.",
  secrets: { apiKey: process.env.ANTHROPIC_API_KEY! }
});

const run = await aex.wait(runId);
console.log(run.status);
console.log(await aex.outputs(runId));
```

For reusable, credential-free configs, use an ordinary function:

```ts
function summarise(topic: string) {
  return {
    model: RunModels.CLAUDE_HAIKU_4_5,
    prompt: `Write a short answer about ${topic}.`
  };
}

const runId = await aex.submitRun({
  ...summarise("agent-first SDK design"),
  secrets: { apiKey: process.env.ANTHROPIC_API_KEY! }
});
```

Or from the shell:

```bash
aex run \
  --api-token "$AEX_API_TOKEN" \
  --anthropic-api-key "$ANTHROPIC_API_KEY" \
  --model claude-haiku-4-5 \
  --prompt "Write a short answer about agent-first SDK design." \
  --follow
```

For a config-file flow, pass `--config <path>` with a run-config JSON file for a single run request (`{ model, system?, prompt, skills?, mcpServers?, environment?, runtimeSize?, timeout?, postHook?, proxyEndpoints?, metadata? }`). Both surfaces hit the same aex backend and operate on the same durable run records. The JSON `model` value is validated against `RUN_MODELS`.

## Runtime controls

`submitRun` also accepts per-run controls that are not secrets:

- `runtimeSize` - a closed managed-runtime preset. Prefer `RuntimeSizes`, e.g. `RuntimeSizes.SHARED_2X_2GB`.
- `timeout` - run deadline as a duration string such as `"30m"` or `"2h"`; bounded server-side.
- `postHook` - optional post-agent verifier, e.g. `{ command: "pnpm test", timeout: "5m", maxTurns: 3, maxChars: 12000 }`. It runs after the agent process exits successfully; failures are sent back to the agent for repair until `maxTurns` is exhausted. Empty `command` is treated as omitted.
- `builtins` - managed-runtime builtin extensions. Omit it to use the default `["developer"]` toolkit. Pass `[]` for a pure-MCP run with no builtins.
- `outputMode` - `"buffered"` by default; pass `"stream"` for per-token assistant text deltas.

## Where things go: customer → primitive mapping

Every kind of thing you want to ship at run time has exactly one right primitive in the SDK. Reach for the right one rather than rolling your own wrapper.

| What you have | Primitive | What it does |
|---|---|---|
| Non-secret paths or config (`BROLL_STORE`, mode flags) | `environment.envVars` | Mounted as `RUNTIME.env` / `RUNTIME.json`; `__KEY__` substitution in agent-facing markdown; echoed back as `run.runtimeManifest.envVars` |
| Upstream HTTPS API keys (TMDB, Brave, Tavily, …) | `ProxyEndpoint` | Credentials live server-side; aex proxy injects them on outbound calls. The key never enters the container. |
| MCP server credentials | `secrets.mcpServers` | Held in run-scoped custody, attached per run |
| Provider API key | `secrets.apiKey` | Required on every `submitRun`; held in run-scoped custody. Carries the BYOK key for the selected `provider` |
| Non-secret reference data folders (transcripts, persona docs, PDFs) | `File.fromPath('./customer-folder/')` | Materialized under `files/<f_id>/<name>` in the run workspace by default and described in the agent-facing instructions |
| Executable skill code (a `.pyz` wrapper, scripts, prompts) | `Skill.fromPath('./skills/my-skill/')` | Mounted under `skills/<name>/`; the bundle's `SKILL.md` is composed into the agent's instructions |
| Agent instructions file | `AgentsMd.fromPath('./AGENTS.md')` | Prepended as the first user turn |

`Skill`, `AgentsMd`, and `File` values are materialized for the run before the first agent turn. `environment.envVars` values surface in runtime metadata and can be referenced by `__KEY__` placeholders in agent-facing markdown.

## Safe retries with `idempotencyKey`

Every `submitRun` call carries an `idempotencyKey`. When omitted the SDK auto-generates a UUID per call. Supplying your own key makes retries deterministic:

| Submit shape | Server response |
| --- | --- |
| New `idempotencyKey` | HTTP 202 — returns the new run id. |
| Same key + identical request body hash | HTTP 200 — returns the original run id. The SDK call resolves with that id. |
| Same key + **different** request body hash | HTTP 409 — body `{ error: { message, code: "idempotency_conflict", details: { existingRunId } } }`. The SDK throws an `HttpError` carrying that body. Use `details.existingRunId` to adopt the pre-existing run, or pick a fresh key. |
| Omitted `idempotencyKey` | A new UUID is generated on every call — repeat submissions create new runs. |

The request hash is computed server-side over the canonical submission JSON (model, prompt, system, environment, skill refs, MCP server descriptors, proxy endpoints, `outputs`, etc.) so reordering JSON keys, adding whitespace, or rotating the inline secret bundle does **not** change the hash. Changing the prompt, model, system, or any other non-secret field does.

Pattern for safe retries:

```ts
const idempotencyKey = crypto.randomUUID();
async function submitWithRetry() {
  for (let attempt = 0; attempt < 3; attempt++) {
    try {
      return await aex.submitRun({
        model: RunModels.CLAUDE_HAIKU_4_5,
        prompt: "...",
        idempotencyKey,
        secrets: { apiKey: process.env.ANTHROPIC_API_KEY! }
      });
    } catch (err) {
      if (err instanceof Error && err.message.includes("network")) continue;
      throw err;
    }
  }
  throw new Error("submitRun failed after retries");
}
```

The same `idempotencyKey` reused with the same body will deterministically resolve to the same run id regardless of how many times the network drops between attempts. Query, stream, wait, or download the run by that id.
