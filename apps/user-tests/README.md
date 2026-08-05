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

`USER_SCENARIOS` contains 47 planned scenario identities across the `packed`,
`local`, `live`, `browser`, `money`, and `operator` suites. At present, the two
packed SDK install scenarios above have real blackbox journey bodies. The
remaining registered tests validate scenario ownership and metadata only; they
must not be treated as journey, network, browser, money, or operator coverage.

`resolveArtifactSelection` accepts either a paired SDK tarball/native CLI
archive or identical exact SDK/CLI versions, and rejects partial or mixed
selection families. Wiring those external selections into journey execution,
native CLI archive testing, and deployed-plane execution remains outstanding.

Useful focused commands are:

```bash
bun run test:user:smoke
bun run test:user:offline
bun run test:user
```

`test:user:smoke` checks the scenario and artifact-selection registries.
`test:user:offline` runs the registry plus packed and local tests. `test:user`
runs every currently committed test file; it does not turn registry-only live
rows into live coverage.
