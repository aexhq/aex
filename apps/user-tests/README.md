# @aexhq/user-tests

Layer-4 test workspace. Exercises a clean install of the current **packed
tarball** (local/offline default, CI, and manual live workflow sessions) or the
exact **published artifact** selected by release workflows, the way a real user
or AI agent would on day one of `npm i @aexhq/sdk`.

This workspace deliberately has **no `workspace:*` dependencies on
`@aexhq/sdk` or other `@aexhq/*` packages**. Every scenario spawns a child process whose
`cwd` is a freshly created tempdir containing a clean install of the
artifact under test. Inside that child, `import "@aexhq/sdk"` resolves
through the install, never through the monorepo symlink.

These tests are the blackbox layer for install, CLI, SDK, package, and
published-artifact behavior.

## Running

For local/offline sessions, no artifact env is required: when neither env below is
set, the fixture packs the current workspace SDK once into a tempdir and
installs that tarball.

CI can still pin the artifact under test by providing **exactly one** of:

| Env | Source under test |
|---|---|
| `AEX_USER_TEST_TARBALL` | absolute path to a Bun-packed tarball |
| `AEX_USER_TEST_VERSION` | published package version, e.g. `0.26.0` |

Then:

```bash
# Offline scenarios (install / cli-bin / sdk-imports / typescript-consumer)
bun run --filter @aexhq/user-tests test:user:offline

# Full suite incl. the live siblings (requires the live target vars below)
bun run --filter @aexhq/user-tests test:user
# or from the repo root:
bun run test:user

# Explicit live suites kept out of the default public sweep.
bun run test:user:fuzz
bun run test:user:providers
bun run test:user:tool-fuzz   # deploy-gated; use manually for reproduction
```

Offline sessions use `vitest.offline.config.ts` and default to 4 parallel test files.
Override with `AEX_USER_TEST_OFFLINE_MAX_WORKERS=<n>`. The default live sweep
uses `AEX_USER_TEST_MAX_WORKERS` and keeps a lower local default; CI prepares one
SDK artifact, splits the live sweep into 50 non-empty shards, and sessions 1 test
file at a time per shard so the hosted-plane pressure stays bounded while the
shard tail gets shorter.

The scenarios live under `test:user` / `test:user:offline`, NOT
`test:unit` — on purpose. The root unit gate (`bun run test:unit`) is a
workspace-recursive runner that invokes every package's `test:unit`
script; because these are named `test:user*`, that gate never sessions them
by default. That matters: they fail loudly when the artifact-under-test
env is unset (by design), so pulling them into the default gate would
break it for everyone. They run only via explicit invocation here and
from `.github/workflows/ci.yml`, `.github/workflows/release.yml`, and
`.github/workflows/live-user-tests.yml`.

Explicit artifact inputs are strict: setting both variables, an invalid version,
or a missing tarball path is a **hard error**, never a silent skip. The
post-publish gates must stay pinned to the exact published version through
`AEX_USER_TEST_VERSION`.

`test:user:tool-fuzz` is the explicit paid tool-capability gate. It uses seeded,
reproducible inputs and real hosted sessions to cover every builtin tool plus custom
tool bundle upload, schema arguments, environment/secret access, result forms,
failure propagation, and redaction. It is excluded from the default public live
sweep, but it is a hard gate in the platform deploy suite, using the supplied
published SDK candidate when present and npm `latest` otherwise.

## CI prerequisites

CI sessions the offline scenarios after the unit gate. The release workflow sessions the
offline scenarios before publish and the live scenarios against the exact
published version after npm visibility. The offline path needs no provider key.

Live scenarios are driven from `.github/workflows/live-user-tests.yml`, against
the configured hosted API. The workflow sessions the default sweep as 50 shards.
They require:

- **Variable `AEX_API_URL`** — hosted API URL.
- **Secret `AEX_API_KEY`** — workspace API key for the selected API URL.
- **Secret `DEEPSEEK_API_KEY`** — customer DeepSeek key. DeepSeek is the
  single RELEASE-GATING provider (SSoT `test/_fixtures/provider.ts`): gating
  shards must never depend on another provider account's billing state.
  `ANTHROPIC_API_KEY` is needed only by the non-gating providers suite
  (`live-on-demand-tests.yml`).

## Live SDK siblings (2026 rebuild)

The `test/live/live-sdk-*.test.ts` files exercise the packed tarball
end-to-end against the configured hosted API. All gating live
coverage sessions DeepSeek-managed (the gate provider); Anthropic-managed is a
per-provider correctness round-trip in `test/live/providers/` (non-gating).

Each test installs the packed tarball into a tempdir, opens a session or sessions a
one-shot `run({ message, apiKeys, ... })`, reads through the session accessors,
and asserts the user's probe string round-trips through a real upstream LLM call.

Required env:

- `AEX_API_URL`
- `AEX_API_KEY`
- `AEX_USER_TEST_TARBALL` *or* `AEX_USER_TEST_VERSION` when testing an
  explicit artifact; if neither is set, the harness packs the checked-out SDK.
- `ANTHROPIC_API_KEY`
- `DEEPSEEK_API_KEY`

Local `.env.local` files should use the canonical variables above.

CI lives in `.github/workflows/live-user-tests.yml`.

`test/live/config-proxyendpoints.user.test.ts` requires a real
`PROXY_OK` round-trip. The test uses a public no-auth upstream and must not be
run against a plane whose `AEX_PROXY_PUBLIC_BASE_URL` does not serve the
dashboard-owned `/api/sessions/:id/proxy/:name` route.

## Heavy full-feature long-session gate

`test/live/live-sdk-heavy-session.test.ts` is the heaviest live scenario:
one deliberately long (multi-minute) session per cell that exercises the
**entire** customer feature surface at once — 3 inline skills, 2 remote
MCP servers, a long `system` message, a multi-step `prompt` (shell +
multiple file writes + read-backs), an AGENTS.md, a custom
`outputs.allowedDirs` path, `builtins`, `environment.envVars` and `metadata` — and validates
**every observable aspect** of the session: the full AG-UI event vocabulary
(incl. `TOOL_CALL_*`, not just text), tool use, skill materialization,
the system/AGENTS.md/prompt channel probes, the outputs round-trip
pipeline, and secret redaction. (Input files / workspace assets are not
exercised — that feature was dropped in the MVP.) Its purpose is to
prove the **app** behaves as expected under a
maximal submission, not to test model capability.

Scope: one DeepSeek-managed cell using the configured
`AEX_USER_TEST_DEEPSEEK_MODEL` or the default `deepseek-v4-flash`.

It is **excluded** from the default `test:user` sweep (see
`vitest.config.ts`) and sessions only via its own entrypoint + config:

```bash
bun run --filter @aexhq/user-tests test:user:heavy   # or: bun run test:user:heavy
```

Required env is identical to the comprehensive scenario
(`AEX_API_URL`, `AEX_API_KEY`, `AEX_USER_TEST_TARBALL` or
`AEX_USER_TEST_VERSION`, and `DEEPSEEK_API_KEY`); model override is
`AEX_USER_TEST_DEEPSEEK_MODEL`.

CI: it sessions via the consolidated on-demand pipeline (see below), not the
default sweep.

## Tool capability fuzz gate

`test/live/live-sdk-tool-capability-fuzz.test.ts` is the paid blackbox tool
matrix. It drives seeded real hosted DeepSeek sessions through a clean SDK install
and covers every builtin tool: file read/write/edit/navigation, process tools,
background bash, web fetch/search, the subagent/subagent_result protocol, and
custom tool bundle upload/execution.

It also covers custom tool schemas, structured arguments, environment and secret
access, result forms, expected tool failures, and redaction. It is **excluded**
from the default `test:user` sweep, sessions in the platform deploy suite via
`aex-platform/.github/workflows/aws-suite.yml`, and remains directly invokable
for reproduction:

```bash
bun run --filter @aexhq/user-tests test:user:tool-fuzz
```

Required env is `AEX_API_URL`, `AEX_API_KEY`, `AEX_USER_TEST_TARBALL` or
`AEX_USER_TEST_VERSION`, and `DEEPSEEK_API_KEY`; model override is
`AEX_USER_TEST_DEEPSEEK_MODEL`.

Because this gate is a single file, its parallelism lever is running the seeded
cells concurrently within the file (each cell is an independent live session with a
uniquely-named runner script and idempotency key). `AEX_USER_TEST_TOOL_FUZZ_CONCURRENCY`
bounds how many cells run at once (default `4`; the deploy suite raises it) — it
is the deliberate cap on concurrent live-run spend and provider rate limits.

## Per-provider correctness suite

`test/live/providers/` holds one minimal round-trip per NON-GATE provider
(`live-sdk-anthropic-managed.test.ts`, `live-sdk-doubao.test.ts`, and future
openai/gemini/mistral/openrouter). Each
proves only that the provider's adapter/routing/registry wiring reaches its
real upstream and returns a valid response — feature depth is covered by the
DeepSeek (openai-chat) gate suites; a non-gate provider (including Anthropic,
anthropic-messages) needs only this connectivity check, not the full scenario
matrix, so the release gate never depends on its account's billing state.

Each file hard-fails when its provider key is absent (e.g. `ANTHROPIC_API_KEY`, `DOUBAO_API_KEY`);
run the suite only in an environment provisioned for the provider matrix. The suite is **excluded**
from the default `test:user` sweep (see `vitest.config.ts`) and sessions via its
own config:

```bash
bun run --filter @aexhq/user-tests test:user:providers
```

## Consolidated on-demand pipeline

The optional suites above — the per-provider correctness matrix and the heavy
full-feature session — are kept out of the default sweep and session together from a
single manual trigger:
`.github/workflows/live-on-demand-tests.yml`. One dispatch prepares the selected
SDK artifact once, then sessions `test:user:providers` and `test:user:heavy` as
independent jobs so you get the full optional signal from one trigger.
`test:user:tool-fuzz` belongs to the platform deploy suite instead, because it
is a deterministic paid gate rather than a non-gating on-demand probe. The
default `live-user-tests.yml` workflow is purely the always-on workhorse sweep
(DeepSeek only) and no longer carries a `ses_heavy` toggle.
