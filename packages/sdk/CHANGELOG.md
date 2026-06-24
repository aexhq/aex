# Changelog

All notable changes to `@aexhq/sdk` are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this package
follows semantic versioning.

## 0.27.0

### Added

- Data surface for reading a workspace's runs and outputs:
  - `AgentExecutor.listRuns(query?)` — paginated, most-recent-first list of the
    token's own runs (`status` / `since` / `limit` / `cursor`).
  - `AgentExecutor.readOutputText(runId, selector, options?)` — byte-capped,
    decoded-UTF-8 read of one output file (default 50 KB, 10 MB ceiling, optional
    `grep`), built for handing run deliverables to an LLM.
  - `createDataTools(client)` — packages the read surface as a vendor-neutral
    LLM tool set (`{ tools, instructions, execute }`). See `examples/data-chat/`.
- Per-provider BYOK: `secrets.apiKeys` is a `{ [provider]: key }` map so subagents
  spawned with a different-family model can inherit the right key. `secrets.apiKey`
  is kept for back-compat (used when no per-provider key matches).

### Changed

- Builtin-tool selection: the old `builtins` token array is replaced by the
  `includeBuiltinTools` boolean (default `true`) plus a unified `tools` list that
  accepts both custom `Tool` bundles and builtin tool-name references
  (`BuiltinTools.<name>`). Cherry-pick a single builtin by listing its name in
  `tools`; disable the standard set with `includeBuiltinTools: false`.

## 0.26.1

### Added

- Run Webhooks: submit a per-run `webhook: { url }` to receive the terminal
  `run.finished` event (signed Standard-Webhooks style). New delivery APIs
  `getRunWebhookDeliveries(runId)` and `redeliverRunWebhook(runId, deliveryId)`,
  plus the `verifyAexWebhook(...)` helper to verify inbound deliveries with no
  extra dependency.

### Changed

- Publishing moved to the Bun release path.
