// The hand-picked release matrix for the live user-test suite.
//
// WHY A PER-FILE COVERAGE TIER
// ----------------------------
// The matrix builder used to fan EVERY collected live file over EVERY declared
// full-coverage runtime kind. On a dev deploy that was 45 files x 2 kinds = 90
// jobs at max-parallel 4, each paying ~3 minutes of checkout/install/pack setup
// to run a test whose recorded median is ~55 seconds. The org is on GitHub Free
// (20 concurrent jobs), so the suite serialised against itself.
//
// The runtime kind decides workspace materialization, in-runtime tool execution,
// journal/park semantics, and capacity. It does NOT decide SDK-side validation,
// auth envelopes, list/search pagination, the webhook delivery ledger, or CLI
// argument handling. Fanning those over a second runtime re-proves shared code.
// So every live file DECLARES its tier here, with a reason:
//
//   runtime-matrix     genuinely runtime-sensitive: one job per full-coverage kind
//   runtime-spotcheck  runtime-matrix files that also own parity verdict cells
//   runtime-agnostic   runtime-incidental: one run, duration-packed into shards
//   on-demand          never in the release gate; its own dispatched lane
//
// A file missing from `LIVE_TEST_COVERAGE` is a hard error, not a default: an
// unclassified new file would otherwise silently inherit whatever the cheapest
// tier happens to be, which is the same defect as a silent drop from the sweep.
// `assertCoverageManifest` runs from `collectTestFiles`, so the sweep, both CI
// matrix builders, and lint all fail on an unclassified or stale entry.
//
// The RUNTIME KINDS themselves are NOT declared here. They belong to the deploy
// gate's scenario ledger (platform `scripts/test-support/runtime-parity/
// scenario-ledger.v1.json`, read through `resolve-runtime-kinds.mjs`), and the
// caller passes them in. This file owns which FILES; the ledger owns which KINDS.

export const COVERAGE_TIERS = Object.freeze([
  "runtime-spotcheck",
  "runtime-matrix",
  "runtime-agnostic",
  "on-demand"
]);

/**
 * Every live file, its coverage tier, the public entry point it drives, and WHY
 * it sits where it does.
 */
export const LIVE_TEST_COVERAGE = Object.freeze({
  // ---- runtime-spotcheck: the two parity anchors, one per entry point -------
  // These files explicitly submit the selected runtime, assert the returned
  // session identity, and emit the public parity verdicts.
  "test/live/edge-cli.user.test.ts": {
    tier: "runtime-spotcheck",
    entryPoint: "cli",
    reason:
      "The CLI entry point end to end on every runtime: start --follow reaches a terminal, then status/wait/events/files/download/delete. The whole shipped-binary day-one surface in one session."
  },
  "test/live/live-sdk-event-stream.test.ts": {
    tier: "runtime-spotcheck",
    entryPoint: "sdk",
    reason:
      "The SDK entry point counterpart: coordinator WS, post-terminal snapshot, and the durable NDJSON archive must agree on every runtime, and the lambda journal projection is a different producer."
  },

  // ---- runtime-matrix: behaviour the execution runtime actually decides -----
  "test/live/live-sdk-comprehensive.test.ts": {
    tier: "runtime-matrix",
    entryPoint: "sdk",
    reason:
      "One maximal-but-short submission (skills, MCP refs, instructions, system, fileCapture) per runtime. The canonical proof that a runtime is wired at all."
  },
  "test/live/edge-event-stream.user.test.ts": {
    tier: "runtime-matrix",
    entryPoint: "sdk",
    reason:
      "Forced mid-turn socket drops must resume exactly-once. Replay cursors are served from the runtime's journal, which lambda projects differently from a container."
  },
  "test/live/edge-instructions-files.user.test.ts": {
    tier: "runtime-matrix",
    entryPoint: "sdk",
    reason:
      "Workspace materialization: files land at real filenames, binary bytes survive zip->asset->unzip, and a /etc mountPath must rebase under /workspace. All runtime-side."
  },
  "test/live/edge-files.user.test.ts": {
    tier: "runtime-matrix",
    entryPoint: "sdk",
    reason:
      "File CAPTURE out of the runtime workspace, then the whole read/find/link/fetch/download selector matrix over what the runtime produced."
  },
  "test/live/edge-skills-tools.user.test.ts": {
    tier: "runtime-matrix",
    entryPoint: "sdk",
    reason:
      "Tool execution inside the runtime: a throwing custom tool stays recoverable, builtin subsets are honoured, and a zero-tool session still completes."
  },
  "test/live/config-envvars.user.test.ts": {
    tier: "runtime-matrix",
    entryPoint: "sdk",
    reason:
      "environment.variables is delivered as a /workspace/RUNTIME.env mount written by the runtime materialization step, so the file is runtime-specific by construction."
  },
  "test/live/config-packages.user.test.ts": {
    tier: "runtime-matrix",
    entryPoint: "sdk",
    reason:
      "environment.packages pre-installs apt and pip entries before the agent runs. The pre-install happens in the runtime image, which differs per kind."
  },
  "test/live/edge-chat-multiturn.user.test.ts": {
    tier: "runtime-matrix",
    entryPoint: "sdk",
    reason:
      "Turn continuity across parks: a second and third turn must see the first turn's context. Lambda parks awaiting_input between turns where a container holds the process."
  },
  "test/live/edge-chat-suspend.user.test.ts": {
    tier: "runtime-matrix",
    entryPoint: "sdk",
    reason:
      "suspend()/resume() park semantics are implemented per runtime; a container is torn down where a lambda session is journalled."
  },

  // ---- runtime-agnostic: control plane, SDK client, and API boundary --------
  "test/live/edge-builtin-web-tools.user.test.ts": {
    tier: "runtime-agnostic",
    entryPoint: "sdk",
    reason:
      "web_search/web_fetch are served by the shared managed egress boundary, not by the runtime; the tool result is produced by the same proxy on every kind."
  },
  "test/live/edge-chat-cancel-launch.user.test.ts": {
    tier: "runtime-agnostic",
    entryPoint: "sdk",
    reason:
      "Cancellation before launch is refused by the control plane; no runtime has started yet, so there is nothing runtime-specific to observe."
  },
  "test/live/edge-chat-cancel-send.user.test.ts": {
    tier: "runtime-agnostic",
    entryPoint: "sdk",
    reason:
      "Cancel-during-send is a coordinator/control-plane transition; the runtime only observes the resulting terminal, which the matrix tier already proves."
  },
  "test/live/edge-chat-concurrency.user.test.ts": {
    tier: "runtime-agnostic",
    entryPoint: "sdk",
    reason:
      "Busy-session rejection of a second concurrent send is enforced at the session record, before dispatch reaches any runtime."
  },
  "test/live/edge-chat-delete.user.test.ts": {
    tier: "runtime-agnostic",
    entryPoint: "sdk",
    reason:
      "Delete is a control-plane record transition and a read-path fence, identical whichever runtime produced the session."
  },
  "test/live/edge-chat-replay.user.test.ts": {
    tier: "runtime-agnostic",
    entryPoint: "sdk",
    reason:
      "replayLast() idempotency keys are resolved at the message route; the runtime never sees the deduped attempt at all."
  },
  "test/live/edge-delete-retention.user.test.ts": {
    tier: "runtime-agnostic",
    entryPoint: "sdk",
    reason:
      "Whether a deleted session's files stay retrievable is an object-store retention and read-fence question, not a runtime one."
  },
  "test/live/edge-errors-validation.user.test.ts": {
    tier: "runtime-agnostic",
    entryPoint: "sdk",
    reason:
      "Auth rejects, 404s, and bounded network failure. No session is ever dispatched, so there is no runtime to vary."
  },
  "test/live/edge-idempotency.user.test.ts": {
    tier: "runtime-agnostic",
    entryPoint: "sdk",
    reason:
      "Body-mismatch conflict on a reused idempotency key is decided by the create route's submission hash, before any runtime is chosen."
  },
  "test/live/edge-lineage-observability.user.test.ts": {
    tier: "runtime-agnostic",
    entryPoint: "sdk",
    reason:
      "Parent/child lineage and cost rollup are read from the session records through the public read surface, which is shared across runtimes."
  },
  "test/live/edge-list-search.user.test.ts": {
    tier: "runtime-agnostic",
    entryPoint: "sdk",
    reason:
      "Post-terminal read consistency and list pagination are control-plane properties measured after the consistency barrier."
  },
  "test/live/edge-mcp-egress.user.test.ts": {
    tier: "runtime-agnostic",
    entryPoint: "sdk",
    reason:
      "SSRF fail-closed and the networking:limited allowlist are enforced by the shared egress proxy and the submission gate, both outside the runtime."
  },
  "test/live/edge-runtime-size.user.test.ts": {
    tier: "runtime-agnostic",
    entryPoint: "sdk",
    reason:
      "Both probes create born-empty idle sessions and delete them without a turn, so no runtime is ever launched to differ."
  },
  "test/live/edge-session-lifecycle.user.test.ts": {
    tier: "runtime-agnostic",
    entryPoint: "sdk",
    reason:
      "Submission, idempotent replay, delete-after, and payload shapes are all decided at the /api/sessions boundary."
  },
  "test/live/edge-session-limits.user.test.ts": {
    tier: "runtime-agnostic",
    entryPoint: "sdk",
    reason:
      "Which override values the server accepts at submit is a resolver question; the size that actually runs is proven by the matrix-tier files."
  },
  "test/live/edge-storage-accrual.user.test.ts": {
    tier: "runtime-agnostic",
    entryPoint: "sdk",
    reason:
      "retainedStorageBytes on the public record is written by the billing accrual path, not by the runtime that produced the bytes."
  },
  "test/live/edge-subagent-mcp-failmodes.user.test.ts": {
    tier: "runtime-agnostic",
    entryPoint: "sdk",
    reason:
      "Child admission validation and the managed egress ceiling for a documented MCP host are both pre-runtime gates."
  },
  "test/live/edge-type-contract.user.test.ts": {
    tier: "runtime-agnostic",
    entryPoint: "sdk",
    reason:
      "Token-usage visibility and the prompt-size ceiling are properties of the served record and the create route, not of the execution runtime."
  },
  "test/live/edge-webhooks.user.test.ts": {
    tier: "runtime-agnostic",
    entryPoint: "sdk",
    reason:
      "Registration, the delivery ledger, HMAC verification, and delivery-time SSRF denial are all control-plane behaviour."
  },
  "test/live/live-default-base-url.test.ts": {
    tier: "runtime-agnostic",
    entryPoint: "sdk",
    reason:
      "One unauthenticated fetch at the hardcoded default hostname. It proves DNS/TLS/routing and never creates a session."
  },
  "test/live/live-sdk-builtin-tools.test.ts": {
    tier: "runtime-agnostic",
    entryPoint: "sdk",
    reason:
      "Whether builtinTools:[] disarms tooling is decided when the manifest is composed; edge-skills-tools carries the per-runtime execution proof."
  },
  "test/live/live-sdk-files-and-failures.test.ts": {
    tier: "runtime-agnostic",
    entryPoint: "sdk",
    reason:
      "Failure surfacing (corrupted skill zip, invalid model, stdio MCP) is rejected before or at admission. edge-files owns the per-runtime round-trip."
  },
  "test/live/live-sdk-mcp-invocation.test.ts": {
    tier: "runtime-agnostic",
    entryPoint: "sdk",
    reason:
      "Proves the model reaches a remote MCP through the shared proxy; the proxy hop is identical on every runtime kind."
  },
  "test/live/live-skill-tool-invocation.test.ts": {
    tier: "runtime-agnostic",
    entryPoint: "sdk",
    reason:
      "Skill selection through the `skills` meta-tool and custom-tool invocation. edge-skills-tools covers the same execution path per runtime."
  },
  "test/live/live-skill-tool-staging.test.ts": {
    tier: "runtime-agnostic",
    entryPoint: "sdk",
    reason:
      "The SKILL.md byte cap and secret redaction are applied by the platform-side load handler, which is shared across runtimes."
  },

  // ---- on-demand: never in the release gate --------------------------------
  "test/live/providers/live-sdk-anthropic-managed.test.ts": {
    tier: "on-demand",
    entryPoint: "sdk",
    reason:
      "The default gate already proves the Anthropic wire shape; this additional managed-model-family round trip is dispatched separately to keep provider billing out of every release."
  },
  "test/live/edge-admission-gates.user.test.ts": {
    tier: "on-demand",
    entryPoint: "sdk",
    reason:
      "Saturates the workspace concurrency cap; needs an isolated low-cap workspace lane or it starves unrelated live assertions."
  },
  "test/live/edge-concurrency-scale.user.test.ts": {
    tier: "on-demand",
    entryPoint: "sdk",
    reason:
      "Declares 10 concurrent session slots and fires a fan of operations. It is a scale/soak probe whose failures are load-shaped, so it informs rather than gates."
  },
  "test/live/live-sdk-heavy-session.test.ts": {
    tier: "on-demand",
    entryPoint: "sdk",
    reason:
      "One deliberately multi-minute maximal submission. Run after the rest pass, never swept into the default lane."
  },
  "test/live/live-api-fuzz.test.ts": {
    tier: "on-demand",
    entryPoint: "sdk",
    reason:
      "Adversarial raw-API fuzzing with its own paid gate and its own budget; probabilistic by construction."
  },
  "test/live/live-sdk-tool-capability-fuzz.test.ts": {
    tier: "on-demand",
    entryPoint: "sdk",
    reason:
      "Deterministic tool-capability fuzz cells with their own paid gate in the platform deploy suite."
  }
});

/** Files the default live sweep never runs; each has an explicit dedicated lane. */
export const ON_DEMAND_FILES = Object.freeze(
  Object.entries(LIVE_TEST_COVERAGE)
    .filter(([, entry]) => entry.tier === "on-demand")
    .map(([file]) => file)
    .sort()
);

/**
 * Every collected live file must be classified, and every classified file must
 * exist. Both directions matter: the first stops a new file from silently
 * inheriting a tier, the second stops the manifest from claiming coverage from a
 * file that was deleted (exactly how a retired suite sat in the parity ledger
 * for a month).
 */
export function assertCoverageManifest(files) {
  const problems = [];
  const collected = new Set(files);
  for (const file of files) {
    const entry = LIVE_TEST_COVERAGE[file];
    if (!entry) {
      problems.push(`${file} has no LIVE_TEST_COVERAGE entry — declare its tier, entry point, and reason`);
      continue;
    }
    if (!COVERAGE_TIERS.includes(entry.tier)) problems.push(`${file} has unknown tier ${entry.tier}`);
    if (entry.entryPoint !== "sdk" && entry.entryPoint !== "cli") {
      problems.push(`${file} must declare entryPoint "sdk" or "cli"`);
    }
    if (typeof entry.reason !== "string" || entry.reason.trim().length < 40) {
      problems.push(`${file} needs a reason that says why its tier is right`);
    }
  }
  for (const file of Object.keys(LIVE_TEST_COVERAGE)) {
    if (!collected.has(file)) problems.push(`${file} is classified but does not exist on disk`);
  }
  if (problems.length > 0) {
    throw new Error(`live-test coverage manifest is incomplete:\n  ${problems.sort().join("\n  ")}`);
  }
  return files;
}

export function coverageFor(file) {
  const entry = LIVE_TEST_COVERAGE[file];
  if (!entry) throw new Error(`${file} has no LIVE_TEST_COVERAGE entry`);
  return entry;
}
