# @aexhq/user-tests

Layer-4 test workspace. Exercises a clean install of the current **packed
tarball** (local/offline default, CI, and manual live workflow runs) or the
exact **published artifact** selected by release workflows, the way a real user
or AI agent would on day one of `bun add @aexhq/sdk`.

This workspace deliberately has **no `workspace:*` dependencies on
`@aexhq/sdk` or other `@aexhq/*` packages**. Every scenario spawns a child process whose
`cwd` is a freshly created tempdir containing a clean install of the
artifact under test. Inside that child, `import "@aexhq/sdk"` resolves
through the install, never through the monorepo symlink.

These tests are the blackbox layer for install, CLI, SDK, package, and
published-artifact behavior.

## Running

For local/offline runs, no artifact env is required: when neither env below is
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

# Explicit live suites kept out of the default sweep.
bun run test:user:fuzz
bun run test:user:providers
```

Offline runs use `vitest.offline.config.ts` and default to 4 file workers.
Override with `AEX_USER_TEST_OFFLINE_MAX_WORKERS=<n>`. The default live sweep
uses `AEX_USER_TEST_MAX_WORKERS` and keeps a lower local default; CI sets it to
4 after selecting a single SDK artifact for all workers.

The scenarios live under `test:user` / `test:user:offline`, NOT
`test:unit` — on purpose. The root unit gate (`bun run test:unit`) is a
workspace-recursive runner that invokes every package's `test:unit`
script; because these are named `test:user*`, that gate never runs them
by default. That matters: they fail loudly when the artifact-under-test
env is unset (by design), so pulling them into the default gate would
break it for everyone. They run only via explicit invocation here and
from `.github/workflows/ci.yml`, `.github/workflows/release.yml`, and
`.github/workflows/live-user-tests.yml`.

Explicit artifact inputs are strict: setting both variables, an invalid version,
or a missing tarball path is a **hard error**, never a silent skip. The
post-publish gates must stay pinned to the exact published version through
`AEX_USER_TEST_VERSION`.

## CI prerequisites

CI runs the offline scenarios after the unit gate. The release workflow runs the
offline scenarios before publish and the live scenarios against the exact
published version after npm visibility. The offline path needs no provider key.

Live scenarios are driven from `.github/workflows/live-user-tests.yml`, against
the configured hosted API. They require:

- **Variable `AEX_API_URL`** — hosted API URL.
- **Secret `AEX_API_TOKEN`** — workspace API token for the selected API URL.
- **Secret `ANTHROPIC_API_KEY`** — customer Anthropic key for the managed
  Anthropic live scenario.
- **Secret `DEEPSEEK_API_KEY`** — customer DeepSeek key for the managed live
  scenarios.

## Live SDK siblings (2026 rebuild)

The `test/live/live-sdk-*.test.ts` files exercise the packed tarball
end-to-end against the configured hosted API. Current CI coverage is
Anthropic-managed for the default-provider proof and DeepSeek-managed for the
broad feature-surface matrix, because those are the provider keys provisioned
for the public live workflow.

Each test installs the packed tarball into a tempdir, spawns
`AgentExecutor.submit({ provider, ... })`, polls `getRun`,
`listEvents`, and `listOutputs`, and asserts the user's probe
string round-trips through a real upstream LLM call.

Required env:

- `AEX_API_URL`
- `AEX_API_TOKEN`
- `AEX_USER_TEST_TARBALL` *or* `AEX_USER_TEST_VERSION`
- `ANTHROPIC_API_KEY`
- `DEEPSEEK_API_KEY`

Local `.env.local` files should use the canonical variables above. The loader
also accepts the legacy local-only `AEX_TEST_DEEPSEEK_API_TOKEN` alias for
`DEEPSEEK_API_KEY` when the canonical name is absent.

CI lives in `.github/workflows/live-user-tests.yml`.

`test/live/config-proxyendpoints.user.test.ts` requires a real
`PROXY_OK` round-trip. The test uses a public no-auth upstream and must not be
run against a plane whose `AEX_PROXY_PUBLIC_BASE_URL` does not serve the
dashboard-owned `/api/runs/:id/proxy/:name` route.

## Heavy full-feature long-session gate

`test/live/live-sdk-heavy-session.test.ts` is the heaviest live scenario:
one deliberately long (multi-minute) session per cell that exercises the
**entire** customer feature surface at once — 3 inline skills, 2 remote
MCP servers, a long `system` message, a multi-step `prompt` (shell +
multiple file writes + read-backs), an AGENTS.md, a custom
`outputs.allowedDirs` path, `builtins`, `environment.envVars` and `metadata` — and validates
**every observable aspect** of the run: the full AG-UI event vocabulary
(incl. `TOOL_CALL_*`, not just text), tool use, skill materialization,
the system/AGENTS.md/prompt channel probes, the outputs round-trip
pipeline, and secret redaction. (Input files / workspace assets are not
exercised — that feature was dropped in the MVP.) Its purpose is to
prove the **app** behaves as expected under a
maximal submission, not to test model capability.

Scope: one DeepSeek-managed cell using the configured
`AEX_USER_TEST_DEEPSEEK_MODEL` or the default `deepseek-v4-flash`.

It is **excluded** from the default `test:user` sweep (see
`vitest.config.ts`) and runs only via its own entrypoint + config:

```bash
bun run --filter @aexhq/user-tests test:user:heavy   # or: bun run test:user:heavy
```

Required env is identical to the comprehensive scenario
(`AEX_API_URL`, `AEX_API_TOKEN`, `AEX_USER_TEST_TARBALL` or
`AEX_USER_TEST_VERSION`, and `DEEPSEEK_API_KEY`); model override is
`AEX_USER_TEST_DEEPSEEK_MODEL`.

CI: it runs via the consolidated on-demand pipeline (see below), not the
default sweep.

## Per-provider correctness suite

`test/live/providers/` holds one minimal round-trip per **extra** provider
(`live-sdk-doubao.test.ts`, and future openai/gemini/mistral/openrouter). Each
proves only that the provider's adapter/routing/registry wiring reaches its
real upstream and returns a valid response — feature depth is already covered
on the two wire shapes by the DeepSeek (openai-chat) and Anthropic
(anthropic-messages) workhorse suites, so a wire-shape-equivalent provider
needs only this connectivity check, not the full scenario matrix.

Each file hard-fails when its provider key is absent (e.g. `DOUBAO_API_KEY`);
run the suite only in an environment provisioned for the provider matrix. The suite is **excluded**
from the default `test:user` sweep (see `vitest.config.ts`) and runs via its
own config:

```bash
bun run --filter @aexhq/user-tests test:user:providers
```

## Consolidated on-demand pipeline

Both optional, expensive suites above — the heavy full-feature session and the
per-provider correctness matrix — are kept out of the default sweep and run
together from a single manual trigger:
`.github/workflows/live-on-demand-tests.yml`. One dispatch runs
`test:user:providers` then `test:user:heavy`; the heavy step runs even if a
provider check failed, so you get the full signal from one trigger. The default
`live-user-tests.yml` workflow is purely the always-on workhorse sweep
(DeepSeek + Anthropic) and no longer carries a `run_heavy` toggle.
