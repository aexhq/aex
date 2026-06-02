# Anthropic API recordings (record-replay)

These fixtures guard Anthropic Managed Agents **response-shape EVOLUTION** — an
inbound `ProviderEvent` field/type renamed, added, or removed under us. They do
**NOT** catch this cycle's seam bugs (those — e.g. the globalThis/undici bugs —
are client-runtime, not inbound shape). See `surface invariants`
§Record-replay.

## Files

| Glob | Committed? | What it is |
|---|---|---|
| `*.raw.json` | **No** (gitignored) | Verbatim live capture. May contain secrets. |
| `*.secret.json` | **No** (gitignored) | Any scratch file that holds a secret. |
| `*.sanitized.json` | **Yes** | Secret-stripped + id/timestamp-normalized fixture. |

`.gitignore` enforces this: `**/test/fixtures/api-recordings/**/*.raw.json` and
`*.secret.json` are ignored; only `*.sanitized.json` and this README are tracked.

## The pipeline

```text
fixtures:record:anthropic   →  <scenario>.raw.json   (LIVE, creds-gated, gitignored)
fixtures:sanitize           →  <scenario>.sanitized.json (offline, committed; FAILS on residual secret)
test:unit:recorded          →  replay the sanitized fixture through the adapter (offline, $0) — per-commit signal
ANTPATH_DRIFT_LIVE=1 pnpm validate 1
                            →  drift-check: live shape == committed fixture shape (OPT-IN; ~60s + tokens)
```

Run from the repo root (or `--filter antpath`):

```bash
pnpm --filter antpath run fixtures:record:anthropic            # LIVE — needs ANTHROPIC_API_KEY
pnpm --filter antpath run fixtures:record:anthropic -- --dry-run
pnpm --filter antpath run fixtures:sanitize                    # offline
pnpm --filter antpath run fixtures:sanitize -- --verify        # CI: re-scan committed fixtures only
```

## Provenance of the committed fixtures

`simple-turn.sanitized.json` is a **real live capture** (`"source":
"live-capture"`, recorded 2026-05-27), then sanitized + id/timestamp-normalized.
Its shape matches what the production code consumes — documented in the hosted
Anthropic-native adapter contract (header + the live-verified event vocabulary)
and asserted by private adapter tests. In particular:

- each event carries a `processed_at` ISO timestamp (the `user.message` echo
  carries none);
- `stop_reason` is an OBJECT (`{ "type": "end_turn" }`), not a bare string —
  `readStopReason` reads `value.type`.

Because the fixture is a real capture, the drift probe
(`scripts/validate/probes/anthropic-event-shape.ts`) treats a live-vs-fixture
SHAPE mismatch as a hard `fail` (the provider genuinely evolved the inbound
contract), and diffs the live response SHAPE against the committed fixture.
Re-capture after an intentional provider change:

```bash
pnpm --filter antpath run fixtures:record:anthropic --scenario simple-turn
pnpm --filter antpath run fixtures:sanitize
```
