---
title: Clients stream handoff
description: Implementation and merge handoff for the Rust-native clients, dashboard, site, and user-test stream.
status: accepted
keywords:
  - sdk
  - cli
  - dashboard
  - site
  - user tests
audience: implementation agents and maintainers
last_verified: 2026-08-05
related:
  - references/rewrite/contracts.md
  - references/rewrite/delivery.md
---

# Clients stream handoff

Plan of record: `references/rust-native-rewrite-2026-07-31/plans/13-clients-dashboard-site.md` in the parent workspace.

## Implemented

- Removed these retired trees without an alias, compatibility surface, or
  replacement npm CLI:

  ```
  packages/contracts
  packages/cli
  apps/docs
  packages/sdk/docs
  scripts/docs
  scripts/openapi
  ```
- Rebuilt `@aexhq/sdk` at `0.50.0` as ESM with zero runtime dependencies. The
  implemented core includes strict workspace/account credential parsing,
  UUIDv7 payload validation, five-region routing, an injectable transport,
  canonical request identities, the total nine-class API error hierarchy,
  route-metadata retry policy, pagination, NDJSON framing, and verified
  download ranges. Sessions reject a model-only override before transport.
- Added the native Rust `aex` CLI at `0.50.0`. It has a closed clap tree,
  mappings checked against `aex_wire::routes::ROUTES`, TOML/environment/flag
  precedence, the 14-row process-exit contract, raw byte output, part/resume
  planning, and deterministic completions for Bash, Zsh, Fish, PowerShell, and
  Elvish.
- Added `apps/site` as a Next 16 static export. Its deterministic generator
  covers all 146 operations in the current route registry and writes only to
  the gitignored `.generated/` tree. The site build does not call a product API.
- Added a stateless Next 16 dashboard core. It has no database, AWS SDK, storage
  adapter, provider-token retention, or durable credential. The implemented BFF
  core denies off-allowlist routes before credential attachment, enforces CSRF,
  creates host-only secure cookies, and exposes bootstrap/session/health routes.
- Replaced the user-test ledger, smoke manifest, and duration file with the
  typed 47-row `USER_SCENARIOS` registry across packed, local, live, browser,
  money, and operator suites. Artifact selection accepts either an exact paired
  SDK/CLI version or a paired tarball/archive, never a mixture.
- Added real packed-SDK journeys for Node and Bun. Each builds one SDK tarball,
  installs it in a clean temporary consumer with its package manager in offline
  mode, resolves the package through that consumer's `node_modules`, checks the
  published root exports, and executes deterministic SDK behavior. These are
  local artifact tests and earn no registry, network, provider, or deployed-plane
  evidence.
- Added fail-loud dashboard and site smoke/e2e companions. Remote targets require
  the explicit `live` feature and use `aex_test_harness::required_env!`; there is
  no skip or missing-environment success path.
- Regenerated `release/test-registry.json` and
  `release/unearned-evidence.json` from package metadata.

## Published peer surface

The TypeScript package entry point is `packages/sdk/src/index.ts` and publishes:

- `@aexhq/sdk::{Aex, SessionsClient, WorkspacesClient, AexOptions,
  SessionCreateRequest}`.
- `@aexhq/sdk::{AccountToken, WorkspaceApiKey, parseCredential, regionalHost,
  ParsedCredential, RegionCode, resolveCentralBaseUrl,
  resolveRegionalBaseUrl, RegionalRoutingOptions}`.
- `@aexhq/sdk::{AexError, AexApiError, AexAuthError, AexConfigError,
  AexConflictError, AexGoneError, AexInternalError, AexNotFoundError,
  AexPreconditionError, AexQuotaError, AexStateError,
  AexStreamProtocolError, AexUnavailableError, AexValidationError,
  apiErrorFromResponse, isRetryable}`.
- `@aexhq/sdk::{RETRY_POLICY, executeWithRetry, FetchTransport, AexTransport,
  WireRequest, WireResponse, Page, Download, DownloadGrant, DownloadRange,
  MAX_SINGLE_GET_BYTES, planDownloadRanges, parseNdjsonFrames}`.

The Rust CLI library publishes:

- `aex_cli::{Cli, Command, CompletionShell, CommandRegistryEntry,
  command_registry, render_completions}`.
- `aex_cli::{config, download, error, output, registry}` as public modules.

The public user-test authority publishes
`apps/user-tests/scenarios.ts::{UserSuite, UserScenario, USER_SCENARIOS}` and
`apps/user-tests/artifacts.ts::{ArtifactSelection, resolveArtifactSelection}`.

## Cross-stream pending types

Closed. The contracts stream supplied the generated TypeScript boundary, the
temporary module that stood in for it is deleted, and no `wire_pending` symbol
survives in `packages/`. The six items it carried now come from:

- `RouteId`, `RouteDescriptor`, `ROUTES` — `packages/sdk/src/generated/routes.ts`,
  emitted by `aex-contract-gen` and pinned to a contract digest.
- `ErrorClass`, `ERROR_METADATA` — `packages/sdk/src/generated/errors.ts`.
- `AexErrorCode` — `packages/sdk/src/transport/errors.ts`.

Only the route and error projections are generated so far. The rest of the
generated surface plan 01 describes is still owed; it is the first peer change
below.

## Required peer changes

- Contracts must land `packages/sdk/src/generated/**` with the route, schema,
  validator, error, scope, and limit exports described by plan 01. That is the
  blocker to generating the complete SDK resource surface and running the
  shared Node/Bun conformance corpus.
- `aex-wire` currently has route metadata but no `aex_wire::client` module. The
  CLI needs the planned generated `WireClient` (or its accepted exact successor)
  before command execution can be composed without inventing protocol logic in
  the UX crate.
- Delivery must add the signed multi-target CLI archive pipeline and supply its
  exact archive descriptor to packed user tests. No archive was signed,
  published, or deployed here.
- Identity/control must expose the browser session exchange and revocation
  clients; infrastructure must grant the dashboard only those two internal
  invocations and supply the Vercel build identity.
- The four operator surfaces remain without a generated operator contract and
  are intentionally absent.

## Deliberate deferrals

Work was landed depth-first in the stream priority order. The following remains
unearned and is recorded as such in the generated evidence registry:

- The SDK resource surface is currently limited to the implemented Sessions and
  Workspaces core and three temporary route descriptors. Content, observations,
  billing, operations, uploads, complete stream recovery, and one-method-per-wire
  operation await the generated TypeScript boundary. The full shared
  conformance/property corpus is not implemented. The packed Node/Bun
  clean-install lanes cover the current root exports and two deterministic
  helpers, not that future complete surface.
- The CLI command tree and policies compile and are tested, but network command
  dispatch, device authorization, actual atomic download I/O, signal handling,
  completion goldens, and signed archive assembly are deferred on the generated
  wire client and delivery descriptor.
- The site currently proves deterministic all-route generation and static
  export, but the complete migrated content tree, per-error/scope/limit pages,
  typedoc catalog, snippet execution, link/frontmatter/a11y gates, and full
  rendered reference corpus remain.
- The dashboard is the authority-free security core, not the complete product:
  OAuth composition, the central/regional passthrough handlers, device and
  invitation flows, shell panels, budget checks, and browser journeys remain.
- The 47 scenario identities and selection contract are present, but only the
  two packed SDK install identities have black-box journey bodies. External
  artifact-selection wiring, the other 45 journeys, signed CLI archive
  verification, deployed-plane execution, cleanup receipts, and routing/shard
  integration remain.
- Live tests were compiled with `--features live` but not executed because no
  deployment or remote descriptor was authorized by this stream.

## Decisions

- Credential parsing validates the embedded UUID version and variant rather
  than only accepting UUID-shaped text.
- Retry is governed only by `ROUTES[id].safeRetry`, reuses one operation
  identity, treats `Retry-After` as a floor, and stops once a streamed byte has
  been observed.
- The native CLI owns UX/configuration/filesystem behavior only; absent generated
  transport composition is a hard error, not a fallback implementation.
- Next is launched through its Node entry point because Bun's Next worker path
  resolution is not compatible with this Next 16 build on Windows. Bun remains
  the package manager, test runner, and generator runtime.
- Remote tests are required-feature targets so default local gates remain
  hermetic while an explicitly selected live lane fails on any missing
  prerequisite.
