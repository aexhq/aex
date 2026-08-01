---
title: Clients stream handoff
description: Implementation and merge handoff for the Rust-native clients, dashboard, site, and user-test stream.
status: implemented-with-deferrals
---

# Clients stream handoff

## Implemented

- Removed the retired `packages/contracts`, `packages/cli`, `apps/docs`,
  `packages/sdk/docs`, `scripts/docs`, and `scripts/openapi` trees without an
  alias, compatibility surface, or replacement npm CLI.
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
  covers all 144 operations in the current route registry and writes only to
  the gitignored `.generated/` tree. The site build does not call a product API.
- Added a stateless Next 16 dashboard core. It has no database, AWS SDK, storage
  adapter, provider-token retention, or durable credential. The implemented BFF
  core denies off-allowlist routes before credential attachment, enforces CSRF,
  creates host-only secure cookies, and exposes bootstrap/session/health routes.
- Replaced the user-test ledger, smoke manifest, and duration file with the
  typed 47-row `USER_SCENARIOS` registry across packed, local, live, browser,
  money, and operator suites. Artifact selection accepts either an exact paired
  SDK/CLI version or a paired tarball/archive, never a mixture.
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

The contracts stream has not yet supplied the TypeScript generated boundary in
this branch. `packages/sdk/src/wire_pending.ts` therefore carries these temporary
items, each marked with the required cross-stream TODO:

- `RouteId`
- `RouteDescriptor`
- `ROUTES`
- `ErrorClass`
- `AexErrorCode`
- `ERROR_METADATA`

They must be replaced by exports from `packages/sdk/src/generated/index.ts` at
merge, and every SDK route reference must then compile against that generated
source without retaining `wire_pending`.

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
  conformance/property corpus and packed Node/Bun clean-install lanes are not
  implemented.
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
- The 47 scenario identities and selection contract are present, but the 47
  black-box journey bodies, clean external installs, signed archive
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
