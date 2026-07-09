# Changelog

All notable changes to `@aexhq/sdk` are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this package
follows semantic versioning.

## 0.40.16

### Fixed

- Increased direct object-storage upload retry tolerance and kept the heavy
  live session gate resilient to pre-create transport failures without retrying
  post-create product assertions.

## 0.40.15

### Fixed

- Retried transient CLI submit transport failures when the session create
  carries an `Idempotency-Key`, while preserving fail-fast behavior for
  non-idempotent POST requests.

## 0.40.14

### Fixed

- Increased the default per-attempt output transfer timeout to 30s so live
  output reads tolerate slower S3 redirect opens under high-concurrency user
  test fanout.

## 0.40.13

### Fixed

- Held gapped terminal WebSocket events briefly so replay backfill can deliver
  lower-sequence text frames before a finished session stream ends.
- Updated settle-consistent event stream docs and tests for the
  `aex.run.settled` barrier emitted by the hosted planes.

## 0.40.12

### Fixed

- Made coordinator WebSocket streams request replay immediately after opening so
  finished-session catch-up no longer depends only on the connection-table
  stream.
- Added stream self-heal timing controls for live diagnostics around quiet
  replay and settle-consistent event streams.

## 0.40.11

### Fixed

- Retried transient transport failures for idempotent CLI read/download
  commands so a single dropped API connection does not fail `aex status`,
  `aex events`, `aex outputs`, or `aex download`.

## 0.40.10

### Changed

- Added `manifest.json` to the `session.events().download()` and
  `session.downloadMetadata()` namespace archives so every public archive zip is
  self-describing.

### Fixed

- Retried transient connection-establishment failures in raw live user-test
  probes without retrying received HTTP responses, keeping high-fanout release
  gates from failing on a single no-response connect timeout.

## 0.40.9

### Fixed

- Added phase-aware output transfer timeout diagnostics so exhausted output
  reads and downloads identify whether they timed out while opening the
  download or while reading the body.
- Strengthened the live output archive gate so structurally valid ZIP downloads
  must also have a manifest with no per-artifact transfer errors.

## 0.40.8

### Fixed

- Bounded output file body transfers and retried idempotent output reads and
  downloads once when a response body stalls, surfacing a structured
  `NETWORK_ERROR` instead of hanging until an outer test or caller timeout.

## 0.40.7

### Fixed

- Routed self-describing dev-plane API keys to the canonical
  `https://dev-api.aex.dev` endpoint when `baseUrl` is omitted, and kept the
  wrong-plane guard symmetric across dev and prd canonical hosts.

## 0.40.6

### Fixed

- Published the current CLI unknown-flag validation so read commands reject
  invalid flags before attempting network calls.
- Fixed the generated SDK package so internal contract helpers are inlined into
  the tarball instead of importing `@aexhq/contracts/internal` at runtime.

## 0.40.5

### Fixed

- Fixed the heavy live release test so it can prefer the complete
  `session.events().list()` transcript without falling back to the shorter
  one-shot result event list.

## 0.40.4

### Fixed

- Added `failureClass` to the typed `Run` record and preserved it when a
  session-backed one-shot result is exposed through the run-compatible view.
- Made unscoped `outputs.search()` scan sessions lazily and stop after the
  requested result limit instead of enumerating every session up front.
- Updated live-session diagnostics for current managed terminals and sparse
  event-stream sequence cursors.

## 0.40.3

### Fixed

- Made `session.events().streamEnvelopes()` complete on managed-session
  terminal events such as `aex.session.succeeded`, matching the current hosted
  session event contract.

## 0.40.2

### Fixed

- Made `aex run --follow` wait briefly for the final session record to reach a
  parked status after terminal stream events, avoiding false failures when the
  managed-plane event stream is ahead of the session mirror.

## 0.40.1

### Fixed

- Fixed the shared fire-and-forget submit transport used by the bundled CLI so
  `aex run --follow` dispatches the first turn instead of creating an idle
  session and waiting for events that can never arrive.

## 0.40.0

### Changed (breaking)

- Removed the CLI package's runtime dependency on SDK client internals from the
  bundled SDK tarball. Host CLI commands now submit and read through public
  contract operations directly.

### Fixed

- Restored SDK validation for the removed `parentRunId` session submission
  field so legacy subagent lineage input fails before any HTTP request.

## 0.39.0

### Added

- Reintroduced Skills as a first-class SDK concept via the top-level `skills`
  option and `Skill.fromDir` / `fromUrl` / `fromFiles` / `fromContent` /
  `fromBytes` factories.
- Added workspace skill lifecycle helpers on `aex.skills` for listing, reading,
  upserting, and deleting named skill bundles.

### Changed (breaking)

- Skills no longer ride the `tools` array. Passing a `Skill` in `tools` now
  fails validation; pass skills through the top-level `skills` option instead.

## 0.38.1

### Added

- Added first-class projected session messages to one-shot and session-turn
  results. `RunResult`, `SessionTurnResult`, and `SessionRunResult` now include
  `messages`, and `session.messages.all()` / `list()` return normalized message
  objects.

### Changed (breaking)

- Slimmed the SDK root surface for launch around the single `Aex` client,
  composition primitives, runtime constants, errors, event guards, and core
  public contracts. Retired convenience/data-tool/debug trace exports are no
  longer available from `@aexhq/sdk`.
- Renamed the SDK constructor credential option to `apiKey` and the environment
  variable to `AEX_API_KEY`, plus `new Aex(apiKey)` and `new Aex(apiKey, options)`
  shortcuts. Pre-launch clean cut — the former `apiToken` option and
  `AEX_API_TOKEN` variable are removed with no compatibility alias.
- `session.messages` is now a callable accessor property, so both
  `session.messages.all()` and older `session.messages().list()` style calls
  resolve through the same message accessor.

## 0.35.0

### Added

- Added built-in retry/backoff for transient API failures, including HTTP
  `429`, `5xx`, `529`, and network errors. The retry loop honors
  `Retry-After`, uses bounded exponential backoff with full jitter, and can be
  tuned or disabled with the client `retry` option.
- Added `AexRateLimitError`, `isRateLimited`, provider fault parsing, and
  `RetryOptions` so callers can handle persistent API/provider throttling
  without parsing raw error bodies.
- Added stable idempotency handling for billable session create/send requests
  and `replayLast` coverage so SDK retries do not double-submit billable turns.
- Added the `machine.spot` run-submission intent for opting into interruptible
  managed capacity.

### Changed (breaking)

- Removed the legacy public `parentRunId` submission field from the typed
  contract. Subagents run through the managed in-process tool path.

## 0.34.0

### Changed (breaking)

- Sessions are now the low-level API and `run()` is the one-shot convenience
  wrapper over them. Open a session with `aex.openSession(options)`, drive it with
  `session.send(...).done()`, and resume it later with
  `aex.openSession(sessionId)`. A `SessionHandle` keeps lifecycle verbs flat
  (`send`, `suspend`, `resume`, `cancel`, `delete`, `refresh`, `wait`, `unit`,
  `download` / `downloadMetadata`) and groups its reads/streams/downloads into
  accessor sub-resources: `session.messages()`, `session.events()`,
  `session.outputs()`, and `session.webhooks()`. Each read accessor exposes
  `list()` / `last()` / `first()`, with `events()` adding `stream()` /
  `streamEnvelopes()` / `archiveLink()` / `download()`, `outputs()` adding
  `read()` / `find()` / `findOne()` / `link()` / `fetch()` / `download()`, and
  `webhooks()` adding `redeliver()`. (e.g. `session.listEvents()` is now
  `session.events().list()`; "get the last message" is
  `await session.messages().last()`.)
- Moved workspace/session reads onto `aex.sessions`: `list(query)` (returns
  `{ sessions, nextCursor }`), `get(id)`, `searchOutputs(query)`, and
  `outputs(id)` — which returns the SAME rich accessor as `session.outputs()`
  (`aex.sessions.outputs(id).list()` / `.read(sel)` / `.download()` / …), so the
  id-addressed workspace reads and the live handle share one accessor convention.
- Renamed submission options: provider keys move to a top-level `apiKeys` map
  (was `secrets.apiKeys`); `prompt` becomes `run({ message })` / `session.send()`;
  `secretEnv` becomes `environment.secrets`; `runtimeSize` becomes `runtime`;
  `timeout` becomes `overrides.timeout`.
- Moved webhooks onto sessions: pass `webhook: { url }` to `openSession` / `run`
  and inspect delivery with `session.webhooks().list()` /
  `session.webhooks().redeliver(id)`. Verify inbound deliveries with
  `verifyAexWebhook`.
- Renamed the data-source chat tools to session vocabulary: `list_runs` →
  `list_sessions`, `get_run` → `get_session`, and their `run_id` argument →
  `session_id` (`list_outputs` / `read_output` / `search_outputs` keep their
  names). `ChatCorpus.runIds` → `sessionIds`.

### Removed

- Removed `submit()` and the entire run-id-addressed client surface
  (`wait` / `stream` / `streamEnvelopes` / `getRun` / `getRunUnit` / `listRuns` /
  `listOutputs` / `readOutputText` / `download*` / `cancel` / `searchOutputs` /
  `getRunWebhookDeliveries` / `redeliverRunWebhook` on the client). Use the
  session-handle and `aex.sessions.*` equivalents.
- Normalized the `chat` surface into sessions: dropped the `ChatClient` /
  `ChatSession` / `ChatTurnStream` aliases (and the `Chat*` option/result types)
  and the `aex.chat` client field — use `SessionClient` / `SessionHandle` /
  `SessionTurnStream` and `aex.sessions`. The `aex chat` CLI corpus command is
  removed too; build a corpus chat programmatically with `createCorpusTools`.
- Removed the `RuntimeSizes` export; use the `Sizes` symbol const (e.g.
  `Sizes.SHARED_0_25X_1GB`).
- Removed the `parentRunId` and `limits` submission options. Subagents run
  in-process; use `overrides` (e.g. `overrides.maxSpendUsd`) for per-session caps.
- Removed the `SubmitOptions`, `RunListPage`, `RunListQuery`, and `RunSummary`
  types.

## 0.33.1

### Fixed

- Made session `send(...).done()` report the terminal session status observed in
  the session event stream when the immediate follow-up session read is still a
  stale active snapshot.

## 0.33.0

### Added

- Added the session-first SDK surface: `Aex`, `openSession()`, `sessions`,
  `chat`, `SessionHandle`, and one-shot `run({ message })` convenience on top of
  resumable sessions.
- Added public session contract types and `/api/sessions` operation helpers for
  create, open, list, message, events, outputs, suspend, resume, cancel, and
  delete.

### Changed (breaking)

- `AgentExecutor.run()` now opens a resumable session and treats the returned
  `runId` as the session id. Use `submit()` plus `wait()` / `stream()` when a
  low-level run-record workflow is required.
- Removed the public `postHook` submission option; validation or repair should
  be expressed as a follow-up session turn instead of an after-run hook.

## 0.32.0

### Changed (breaking)

- Removed the `RunModels` back-compat alias. Use the canonical `Models` constants
  (provider-neutral model ids); `RunModels` was a 1:1 alias of `Models`.
- Removed the deprecated `SignedOutputLink` type alias. Use `OutputLink`.
- Removed the legacy DeepSeek model ids `deepseek-chat` / `deepseek-reasoner`
  (`Models.DEEPSEEK_CHAT` / `Models.DEEPSEEK_REASONER`). Use `deepseek-v4-flash`
  (`Models.DEEPSEEK_V4_FLASH`, the prior `deepseek-chat` route) or
  `deepseek-v4-pro` (`Models.DEEPSEEK_V4_PRO`).
- Removed unused internal upload transports from the public client classes.

## 0.31.0

### Changed (breaking)

- Removed the retired public runtime, region, upload, and credential-mode choices
  from SDK and CLI submission paths. The public surface now targets the single
  hosted managed runtime, asset-backed skills, and `secrets.apiKeys`.
- Updated provider/runtime capability docs and validation to the current
  submission-parser plus managed-execution model.

### Fixed

- Hardened CLI config validation coverage so invalid skill asset ids are
  rejected before any run submission request is sent.
- Added installed-package blackbox coverage for the major SDK submit input
  shapes, builder edge cases, secret redaction, and no-network validation paths.

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
  - `aex login` / `aex logout` / `aex auth status` — persist your API key and
    default `--aex-url` to a `0600` config file so commands stop re-passing
    `--api-key`. `login` validates the token against `whoami` before writing (a
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
