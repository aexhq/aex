# Changelog

All notable changes to `@aexhq/sdk` are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this package
follows semantic versioning.

## 0.28.1

### Fixed

- Live event stream no longer hangs on a silently half-open coordinator socket.
  A stalled WebSocket (no close/error, no frames) previously blocked the async
  iterator forever and could MISS a `RUN_FINISHED`/`RUN_ERROR` that was already
  persisted server-side. The consumer now:
  - sends a lightweight keep-alive ping the coordinator auto-responds to (no
    durable-object wake), and
  - runs an idle watchdog (default 45s) that, on no inbound frame, treats the
    socket as dead and reconnects — resuming from the last cursor, which replays
    the terminal exactly-once.
  Tunable via the internal stream options `idleTimeoutMs` / `pingIntervalMs`;
  a legitimately quiet run is kept alive by the ping/pong and does not reconnect.

## 0.28.0

### Added

- Per-run lineage limit overrides via `SubmitOptions.limits`:
  - `limits.maxConcurrentChildRuns` — the max LIVE (non-terminal) child runs
    allowed under a lineage root before a spawn is rejected with
    `child_cap_exceeded` (platform default `1000`, hard ceiling `4096`).
  - `limits.maxSubagentDepth` — the deepest subagent lineage the run may spawn
    before a child submit is rejected with `depth_exceeded` (platform default and
    hard ceiling `5`).
  Both fields are optional; an absent field uses the platform default. The cap is
  resolved ONCE at the root submit and governs the whole subtree. `submit()`
  validates the override client-side (shape / positivity / allow-list) and throws
  `AexError(RUN_CONFIG_INVALID)` before any asset upload — clamping to the
  per-workspace and platform ceilings is the server's job, so an in-range value
  here is not a guarantee it won't be lowered server-side.

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
