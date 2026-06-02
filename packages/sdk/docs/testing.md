---
title: Testing
---

# Testing

antpath uses test-first development.

Workspace-wide commands:

```text
pnpm test                                       # unit, all packages, deterministic
pnpm --filter antpath run test:unit:recorded    # unit + replayed provider fixtures
pnpm test:integration                           # live external systems — no skip flags
pnpm test:e2e                                   # full top-to-bottom flows against live services
pnpm test:user                                  # published antpath package (offline + live)
```

Unit tests are deterministic and may use fakes or sanitized recorded snapshots. Integration tests run live external systems without any skip flag; if credentials are missing they fail loudly. Live e2e and user-live tests require `.env.local` (or runner-provided env) to include `ANTHROPIC_API_KEY` and any other live target vars.

## Recorded provider fixtures (record-replay)

These guard provider/runtime **response-shape evolution** — an inbound
`ProviderEvent` field/type renamed, added, or removed under us. They do NOT catch
outbound-string or client-runtime seam bugs (see
`surface invariants` §Record-replay).

```text
pnpm --filter antpath run fixtures:record:anthropic   # LIVE capture (needs ANTHROPIC_API_KEY); --dry-run to preview
pnpm --filter antpath run fixtures:sanitize           # offline: strip secrets + normalize; fails on residual secret
pnpm --filter antpath run fixtures:sanitize -- --verify  # CI: re-scan committed fixtures only
pnpm --filter antpath run test:unit:recorded          # replay the committed sanitized fixture, offline + $0
```

The pipeline and the security gate live in `scripts/`:

- `scripts/record-api-fixtures.ts` writes a gitignored `*.raw.json` (may contain
  secrets — never committed).
- `scripts/sanitize-api-fixtures.ts` strips every secret-**shaped** value via the
  public value-agnostic redactor, normalizes ids/timestamps, and **exits
  non-zero if any committed `*.sanitized.json` still contains a secret-shaped
  value**.
- Only `*.sanitized.json` is committed. `*.raw.json` and `*.secret.json` are
  gitignored.
- The per-commit signal is the OFFLINE replay (`test:unit:recorded`, $0). The
  Tier-1 drift check (`ANTPATH_DRIFT_LIVE=1 pnpm validate 1`) is **opt-in** — it
  re-hits live Anthropic (~60s + tokens) and asserts the live response SHAPE
  matches the committed fixture (SHAPE, not values). Without the opt-in flag the
  probe reports `error` so an un-run seam is never read as green.

The first committed fixture (`test/fixtures/api-recordings/simple-turn.sanitized.json`)
is a labelled **derived seed**, not yet a real live capture — see that
directory's `README.md`.
