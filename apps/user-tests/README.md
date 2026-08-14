# @aexhq/user-tests

Blackbox tests for the public SDK, native CLI, dashboard, and site. The typed
scenario authority is [`scenarios.ts`](scenarios.ts); artifact selection is
defined in [`artifacts.ts`](artifacts.ts).

## Offline tests

From the repository root:

```bash
bun run test:user:offline
```

The implemented packed SDK tests build and pack the checked-out
`@aexhq/sdk`, then create separate temporary consumers for Node and Bun. The
Node consumer installs the tarball with npm in offline mode; the Bun consumer
installs it with Bun in offline mode. Both disable package lifecycle scripts,
resolve `@aexhq/sdk` through the temporary consumer's `node_modules`, import
the published root entry, check its runtime exports, and execute deterministic
routing and download-range behavior. Temporary tarballs, caches, installs, and
probes are removed when the suite finishes.

These tests prove clean local tarball installation and the current SDK root
surface. They do not contact a registry, product API, provider, or deployed
plane, and they do not prove publication or live behavior.

## Current scope

`USER_SCENARIOS` contains 58 scenario identities across the `packed`, `local`,
`live`, and `browser` suites. Packed and local tests exercise built artifacts
without a plane. The live file contains deployed-dev journey bodies for
latest-only files and uploads, frozen mounts, `storage.persist`, message and
telemetry NDJSON streams, native structured output, sandbox opt-out,
subagents, both MCP transports, the seven provider candidates, and essential
billing, explicit termination, and a 24-turn rolling-context journey. Browser
rows remain ownership metadata until the dashboard runner drives them.

`resolveArtifactSelection` accepts either a paired SDK tarball/native CLI
archive or identical exact SDK/CLI versions, and rejects partial or mixed
selection families. Wiring those external selections into every journey and
native CLI archive testing remains outstanding; deployed-dev bodies execute
directly when their evidence environment is supplied.

## Dev live journeys

Run the deployed journeys only against a disposable dev evidence account:

```bash
bun run test:user:live
```

The common inputs are `AEX_API_URL`, `AEX_API_KEY`,
`AEX_LIVE_PROVIDER`, `AEX_LIVE_MODEL`, `AEX_LIVE_PROVIDER_API_KEY`, and
`AEX_LIVE_TOPUP_AMOUNT_CENTS`. The seven-provider journey reads
`AEX_LIVE_PROVIDER_CASES`, a JSON array of `{ provider, model, apiKey }` rows.
`AEX_LIVE_LONG_CONTEXT_TURNS` optionally selects 12–64 turns and defaults to
24.

Remote MCP uses `AEX_LIVE_REMOTE_MCP_URL`, optional JSON
`AEX_LIVE_REMOTE_MCP_HEADERS`, and the
`AEX_LIVE_REMOTE_MCP_{SERVER,TOOL,ARGUMENTS,EXPECTED}` fixture values. Sandbox
MCP uses JSON `AEX_LIVE_SANDBOX_MCP_TRANSPORT` plus the equivalent
`AEX_LIVE_SANDBOX_MCP_*` values. Arguments and the sandbox transport are JSON;
secrets stay in the evidence environment and never in source.

Set `AEX_RELEASE_EVIDENCE_MODE=inventory` to compile and enumerate every live
body without contacting a plane. A real evidence run also supplies the release
hygiene variables consumed by the cleanup ledger.

Useful focused commands are:

```bash
bun run test:user:smoke
bun run test:user:offline
bun run test:user
```

`test:user:smoke` checks the scenario and artifact-selection registries.
`test:user:offline` runs the registry plus packed and local tests. `test:user`
runs the offline and browser-owned files. Live tests are a separate explicit
command because they create sessions, file state, Stripe hosted checkouts, and
provider traffic in dev.
