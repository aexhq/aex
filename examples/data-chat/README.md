# Data-source chat (SDK-only)

An interactive chat over your aex workspace — list runs, inspect a run, and read
its output files — driven by Claude with **your own** Anthropic key.

It is built on nothing but the **public SDK**. The single aex dependency is
`@aexhq/sdk`; the chat's tools come from `createDataTools(client)`, which only
calls public read methods (`listRuns`, `getRun`, `listOutputs`, `readOutputText`).
So the assistant can reach your runs and their captured outputs **and nothing
else** — there is no path to internal or operator data, by construction.

## How it works

```
 your terminal ──▶ chat.mjs ──▶ Anthropic Messages API (your key, BYOK)
                      │              │  tool_use
                      │              ▼
                      └─ createDataTools(client).execute(name, input)
                                     │  (public @aexhq/sdk read methods only)
                                     ▼
                            aex API  (scoped to your workspace token)
```

- **Search-then-fetch.** `list_runs` / `list_outputs` return lean references and
  metadata; only `read_output` returns file content, and it is byte-capped
  (default 50 KB), so a large deliverable never floods the context window.
- **Two keys, one boundary.** The workspace token scopes data access; the
  Anthropic key pays for the model. Both live only in this process.

## Run

```bash
export AEX_API_TOKEN=...        # your workspace token
export ANTHROPIC_API_KEY=...    # your Anthropic key (BYOK)
# export AEX_BASE_URL=https://api.aex.dev   # optional API plane override
# export AEX_CHAT_MODEL=claude-sonnet-4-6   # optional model override

npm install
node chat.mjs
```

Then ask things like:

- `list my last 5 runs`
- `which of my recent runs failed?`
- `summarize the report from my most recent succeeded run`
- `show me the first 100 lines of output.md from run <id>`

## The tools (vendor-neutral)

`createDataTools(client)` returns `{ tools, instructions, execute }`. `tools` is a
plain JSON-Schema tool set (the shape every major LLM tool API accepts), so the
same adapter works beyond Anthropic:

| Tool | Returns |
|---|---|
| `list_runs` | run summaries + `nextCursor` (no prompts/outputs) |
| `get_run` | one run's status / timing / cost |
| `list_outputs` | a run's output files (metadata only) |
| `read_output` | one file as byte-capped text (`truncated` flag, optional `grep`) |
