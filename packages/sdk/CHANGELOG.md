# Changelog

All notable changes to `@aexhq/sdk` are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this package
follows semantic versioning.

## 0.30.0

### Added

- Per-run spend cap via `limits.maxSpendUsd` (`SubmitOptions.limits`). A positive
  USD amount that bounds total spend for a single run: once the run would exceed
  the cap it is stopped. An absent field means no per-run cap (the run is still
  bounded by its `timeout` and any workspace-level cap). As with the other
  `limits` fields, `submit()` validates shape/positivity client-side and the
  server resolves the value against the workspace and platform ceilings.
- Read-only chat over a fixed corpus of runs, built only on the public read
  surface:
  - `AgentExecutor.searchOutputs(query?)` — search across the token's own run
    outputs, returning lean references (pair with `readOutputText` to fetch the
    matching content).
  - `createCorpusTools(client, corpus, options?)` — packages the corpus read
    surface (`list_runs` / `list_outputs` / `read_output` / search) as a
    vendor-neutral LLM tool set scoped to an explicit run allow-list or a
    `listRuns` filter; every tool refuses a run outside the resolved corpus.
  - `aex chat` — a read-only, multi-run chat CLI over a corpus that uses your own
    provider key plus the corpus read tools. See `examples/data-chat/`.
- CLI host commands for auth and live run inspection:
  - `aex login` / `aex logout` / `aex auth status` — persist your API token and
    default `--aex-url` to a `0600` config file so commands stop re-passing
    `--api-token`. `login` validates the token against `whoami` before writing (a
    bad token is never persisted) and the token value is never printed.
  - `aex tail <run-id>` — live, human-readable follow over the coordinator
    envelope stream (the low-latency equivalent of `events --follow`), with
    `--json` / `--filter` / `--logs` and a jump-to-failure line on `RUN_ERROR`.
  - `aex inspect <run-id>` — one-shot full-timeline render with a header, a
    settle-consistent timeline, a jump-to-failure line, and a cost/usage footer.

## 0.29.0

### Changed (breaking)

- **Regions renamed to product tokens.** `RUN_REGIONS` / `RunRegions` /
  `RunRegion` / `parseRunRegion` are now `REGIONS` / `Regions` / `Region` /
  `parseRegion`, and the accepted tokens are `eu-west` / `us-west` /
  `ap-northeast` (was `lhr` / `iad` / `sfo` / `bom`). `iad` (`us-east`) is
  dropped with no replacement. Update any `region:` value and any
  `RunRegions.*` / `type RunRegion` import.
- **`runtimeSize` tokens right-sized to real boxes.** The preset set is now the
  six managed runtime tiers `shared-0.06x-256mb` / `shared-0.25x-1gb` /
  `shared-0.5x-4gb` / `shared-1x-6gb` / `shared-2x-8gb` / `shared-4x-12gb`
  (fractional vCPU). The default machine changes from `shared-1x-128mb` to
  `shared-0.25x-1gb` (0.25 vCPU / 1 GiB), which changes default run memory
  headroom and cost. `RuntimeSizes.*` accessor keys are renamed accordingly.

## 0.28.1

### Fixed

- Live event stream no longer hangs on a silently half-open coordinator socket.
  A stalled WebSocket (no close/error, no frames) previously blocked the async
  iterator forever and could MISS a `RUN_FINISHED`/`RUN_ERROR` that was already
  persisted server-side. The consumer now:
  - sends a lightweight keep-alive ping the coordinator auto-responds to
    (without forcing an extra server-side wake), and
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
  spawned with a different-family model can inherit the right key.

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
