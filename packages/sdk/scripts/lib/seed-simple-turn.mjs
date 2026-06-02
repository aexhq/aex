// One-off generator: emit simple-turn.sanitized.json from the live-verified
// canonical Managed Agents vocabulary, run through the SAME sanitize+normalize
// pipeline the real recorder uses. Kept in-tree so the seed is reproducible and
// is genuinely the pipeline's output, not hand-typed. Run with `tsx` from this
// dir; safe to delete — it does no network I/O.
//
//   tsx scripts/lib/seed-simple-turn.mjs
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { buildSanitizedFixture } from "./fixtures.ts";

const PROBE = "fixture-marker-7f3a2c";
const BASE = Date.parse("2026-05-25T12:00:00.000Z");
const iso = (offsetSec) => new Date(BASE + offsetSec * 1000).toISOString();

// DERIVED SEED — built from the documented simple-turn vocabulary that the hosted
// adapter + provider already consume, which was itself written from real
// observation. It is NOT a fresh live capture; field shapes
// match what the code actually reads, so the offline replay is faithful:
//   - the cursor/timestamp field is `created_at` (the provider polls
//     `created_at[gt]`; the documented event is `{ id, type, created_at, ... }`),
//   - `stop_reason` is a STRING ("end_turn"/"error"/"max_tokens" — the shape
//     handleStatusIdle reads and adapter.test.ts asserts).
// The extra sub-fields below (agent_name, session_thread_id, is_error,
// model_request_start_id, the cache_* usage counts) are ILLUSTRATIVE
// placeholders the adapter passes through verbatim; they are plausible but
// UNVERIFIED against the wire. Two event types the provider emits
// (`session.error`, `session.status_rescheduled`) are intentionally NOT invented.
// Because this is a labelled placeholder, the drift probe treats it as
// non-authoritative: a live-vs-fixture mismatch is reported as `error` ("seed it
// for real"), never a false `fail` and never a false green. Replace it with a
// real `fixtures:record` capture to make it authoritative.
const raw = {
  scenario: "simple-turn",
  source: "derived-seed",
  capturedAt: iso(0),
  events: [
    { id: "evt_user_0", type: "user.message", content: [{ type: "text", text: `Output verbatim and nothing else: ${PROBE}` }] },
    { id: "evt_run_1", type: "session.status_running", created_at: iso(1) },
    { id: "evt_thr_2", type: "session.thread_status_running", created_at: iso(1), agent_name: "fixture-agent", session_thread_id: "sthr_abc123" },
    { id: "evt_mrs_3", type: "span.model_request_start", created_at: iso(2) },
    { id: "evt_think_4", type: "agent.thinking", created_at: iso(3), content: [{ type: "text", text: "The user wants a verbatim echo." }] },
    { id: "evt_msg_5", type: "agent.message", created_at: iso(4), content: [{ type: "text", text: PROBE }] },
    { id: "evt_mre_6", type: "span.model_request_end", created_at: iso(5), is_error: false, model_request_start_id: "evt_mrs_3", model_usage: { input_tokens: 24, output_tokens: 11, cache_read_input_tokens: 0, cache_creation_input_tokens: 0 } },
    { id: "evt_thr_7", type: "session.thread_status_idle", created_at: iso(6), stop_reason: "end_turn" },
    { id: "evt_idle_8", type: "session.status_idle", created_at: iso(6), stop_reason: "end_turn" }
  ]
};

const fixture = buildSanitizedFixture(raw);
const out = fileURLToPath(new URL("../../test/fixtures/api-recordings/simple-turn.sanitized.json", import.meta.url));
writeFileSync(out, JSON.stringify(fixture, null, 2) + "\n", "utf8");
console.log(`wrote ${out} (${fixture.events.length} events, source=${fixture.source})`);
