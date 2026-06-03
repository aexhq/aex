# @antpath/user-tests

Layer-4 test workspace. Exercises a clean install of the current **packed
tarball** (local/offline default and pre-publish gate inside `publish.yml`) or
the **published artifact** (post-publish matrix and `workflow_dispatch`) the
way a real user or AI agent would on day one of `npm install antpath`.

This workspace deliberately has **no `workspace:*` dependencies on
`antpath` or `@antpath/*`**. Every scenario spawns a child process whose
`cwd` is a freshly created tempdir containing a clean install of the
artifact under test. Inside that child, `import "antpath"` resolves
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
| `ANTPATH_USER_TEST_TARBALL` | absolute path to a `pnpm pack` tarball (pre-publish) |
| `ANTPATH_USER_TEST_VERSION` | published npm version, e.g. `0.12.3` (post-publish) |

Then:

```bash
# Offline scenarios (install / cli-bin / sdk-imports / typescript-consumer)
pnpm --filter @antpath/user-tests run test:user:offline

# Full suite incl. the live siblings (requires the live target vars below)
pnpm --filter @antpath/user-tests run test:user
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
from `publish.yml` / `rebuild-live.yml`.

Explicit artifact inputs are strict: setting both variables, an invalid version,
or a missing tarball path is a **hard error**, never a silent skip. The
post-publish gate stays pinned to the exact published version through
`ANTPATH_USER_TEST_VERSION`.

## CI prerequisites

The pre-publish gate inside `publish.yml` and the post-publish matrix
job run the offline scenarios only — neither needs an Anthropic key.

Live scenarios are driven from `publish.yml` post-publish, against the
deployed `api.antpath.ai` hosted API. They require:

- **Secret `USER_TEST_ANTHROPIC_KEY`** — a real Anthropic API key, so
  the submitted run can drive Claude end-to-end.
- **Secret `USER_TEST_API_TOKEN`** + variable `USER_TEST_ANTPATH_URL`
  — a workspace API token whose scope set covers `runs:read,write`,
  `outputs:read`, and `skills:read,write,delete`. The live scenarios
  probe the token's scopes on `beforeAll` and skip-with-warning when
  required scopes are absent, so an incomplete secret doesn't fail
  the deploy chain.

## Live SDK siblings (2026 rebuild)

`test/live/live-sdk-deepseek.test.ts` and
`test/live/live-sdk-anthropic-managed.test.ts` exercise the published
tarball end-to-end against the deployed `api.antpath.ai` hosted API —
one file per managed provider dispatch cell.

Each test installs the packed tarball into a tempdir, spawns
`AntpathClient.submitRun({ provider, ... })`, polls `getRun`,
`listEvents`, and `listOutputs`, and asserts the user's probe
string round-trips through a real upstream LLM call.

Required env (all three):

- `ANTPATH_API_URL`
- `ANTPATH_API_TOKEN`
- `ANTPATH_USER_TEST_TARBALL` *or* `ANTPATH_USER_TEST_VERSION`
- `DEEPSEEK_API_KEY` (deepseek file)
  / `ANTHROPIC_API_KEY` (anthropic file)

Local `.env.local` files may still use the legacy names
`ANTPATH_LIVE_API_BASE`, `ANTPATH_LIVE_API_TOKEN`,
`ANTPATH_USER_TEST_DEEPSEEK_KEY`, and `ANTPATH_USER_TEST_ANTHROPIC_KEY`;
the test loader aliases them to the canonical variables above.

CI lives in `.github/workflows/rebuild-live.yml` (`sdk-live` job).

`test/live/config-proxyendpoints.user.test.ts` requires a real
`PROXY_OK` round-trip. The test uses a public no-auth upstream and must not be
run against a plane whose `ANTPATH_PROXY_PUBLIC_BASE_URL` does not serve the
dashboard-owned `/api/runs/:id/proxy/:name` route.

## Heavy full-feature long-session gate

`test/live/live-sdk-heavy-session.test.ts` is the heaviest live scenario:
one deliberately long (multi-minute) session per cell that exercises the
**entire** customer feature surface at once — 3 inline skills, 2 remote
MCP servers, a long `system` message, a multi-step `prompt` (shell +
multiple file writes + read-backs), an AGENTS.md, a custom `outputDirs`
path, `builtins`, `environment.envVars` and `metadata` — and validates
**every observable aspect** of the run: the full AG-UI event vocabulary
(incl. `TOOL_CALL_*`, not just text), tool use, skill materialization,
the system/AGENTS.md/prompt channel probes, the outputs round-trip
pipeline, and secret redaction. (Input files / workspace assets are not
exercised — that feature was dropped in the MVP.) Its purpose is to
prove the **app** behaves as expected under a
maximal submission, not to test model capability.

Scope: Anthropic + DeepSeek, one model each. Two managed cells —
`managed/deepseek`, `managed/anthropic`.

It is **excluded** from the default `test:user` sweep (see
`vitest.config.ts`) and runs only via its own entrypoint + config:

```bash
pnpm --filter @antpath/user-tests run test:user:heavy   # or: pnpm test:user:heavy
```

Required env is identical to the comprehensive scenario
(`ANTPATH_API_URL`, `ANTPATH_API_TOKEN`, `ANTPATH_USER_TEST_TARBALL` or
`ANTPATH_USER_TEST_VERSION`, and the two `ANTHROPIC_API_KEY` /
`DEEPSEEK_API_KEY` provider keys); model overrides are
`ANTPATH_USER_TEST_{ANTHROPIC,DEEPSEEK}_MODEL`.

CI: it runs as a **hard gate** in manual `rebuild-live.yml` canary runs
(`sdk-live` job).
