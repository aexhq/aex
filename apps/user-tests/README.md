# @aexhq/user-tests

Layer-4 test workspace. Exercises a clean install of the current **packed
tarball** (local/offline default, CI, and the release pre-publish gate)
or the **published artifact** (release post-publish checks and manual live
workflow runs) the way a real user or AI agent would on day one of
`npm install @aexhq/sdk`.

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
| `AEX_USER_TEST_TARBALL` | absolute path to a `pnpm pack` tarball (pre-publish) |
| `AEX_USER_TEST_VERSION` | published npm version, e.g. `0.12.3` (post-publish) |

Then:

```bash
# Offline scenarios (install / cli-bin / sdk-imports / typescript-consumer)
pnpm --filter @aexhq/user-tests run test:user:offline

# Full suite incl. the live siblings (requires the live target vars below)
pnpm --filter @aexhq/user-tests run test:user
# or from the repo root:
pnpm test:user
```

The scenarios live under `test:user` / `test:user:offline`, NOT
`test:unit` — on purpose. The root unit gate (`pnpm test:unit`) is a
workspace-recursive runner that invokes every package's `test:unit`
script; because these are named `test:user*`, that gate never runs them
by default. That matters: they fail loudly when the artifact-under-test
env is unset (by design), so pulling them into the default gate would
break it for everyone. They run only via explicit invocation here and
from `.github/workflows/ci.yml`, `.github/workflows/release.yml`, and
`.github/workflows/live-user-tests.yml`.

Explicit artifact inputs are strict: setting both variables, an invalid version,
or a missing tarball path is a **hard error**, never a silent skip. The
post-publish gate stays pinned to the exact published version through
`AEX_USER_TEST_VERSION`.

## CI prerequisites

CI runs the offline scenarios after the unit gate. The release workflow
runs the same offline scenarios against the packed tarball before publish and
against the published npm version after publish. Neither path needs a provider
key.

Live scenarios are driven from `.github/workflows/live-user-tests.yml`, against
the configured hosted API. They require:

- **Variable `AEX_API_URL`** — hosted API URL.
- **Secret `AEX_API_TOKEN`** — workspace API token for the selected API URL.
- **Secret `DEEPSEEK_API_KEY`** — customer DeepSeek key for the managed live
  scenarios.

## Live SDK siblings (2026 rebuild)

The `test/live/live-sdk-*.test.ts` files exercise the published tarball
end-to-end against the configured hosted API. Current CI coverage is
DeepSeek-managed because that is the provider key provisioned for the public
live workflow.

Each test installs the packed tarball into a tempdir, spawns
`AgentExecutor.submitRun({ provider, ... })`, polls `getRun`,
`listEvents`, and `listOutputs`, and asserts the user's probe
string round-trips through a real upstream LLM call.

Required env (all three):

- `AEX_API_URL`
- `AEX_API_TOKEN`
- `AEX_USER_TEST_TARBALL` *or* `AEX_USER_TEST_VERSION`
- `DEEPSEEK_API_KEY`

Local `.env.local` files must use the canonical variables above; the test
loader does not provide compatibility aliases.

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
`AEX_USER_TEST_DEEPSEEK_MODEL` or the default `deepseek-chat`.

It is **excluded** from the default `test:user` sweep (see
`vitest.config.ts`) and runs only via its own entrypoint + config:

```bash
pnpm --filter @aexhq/user-tests run test:user:heavy   # or: pnpm test:user:heavy
```

Required env is identical to the comprehensive scenario
(`AEX_API_URL`, `AEX_API_TOKEN`, `AEX_USER_TEST_TARBALL` or
`AEX_USER_TEST_VERSION`, and `DEEPSEEK_API_KEY`); model override is
`AEX_USER_TEST_DEEPSEEK_MODEL`.

CI: it runs as a **hard gate** when the manual
`.github/workflows/live-user-tests.yml` workflow is dispatched with
`run_heavy=true`.
