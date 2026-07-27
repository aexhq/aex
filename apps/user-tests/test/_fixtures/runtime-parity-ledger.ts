export const PARITY_RUNTIME_KINDS = ["container", "spot_container", "lambda"] as const;
export type ParityRuntimeKind = (typeof PARITY_RUNTIME_KINDS)[number];
export type RuntimeKind = ParityRuntimeKind;
export type PublicTestLayer = "e2e" | "user";
export type PublicEntryPoint = "sdk" | "cli";

export interface PublicRuntimeParityScenario {
  readonly id: string;
  readonly requirement: string;
  readonly runtimes: readonly ParityRuntimeKind[];
  readonly layers: Readonly<Record<PublicTestLayer, readonly PublicEntryPoint[]>>;
  readonly sourceFiles: readonly string[];
}

export interface ScenarioCell {
  readonly scenarioId: string;
  readonly layer: PublicTestLayer;
  readonly entryPoint: PublicEntryPoint;
  readonly runtime: RuntimeKind;
}

export interface ScenarioVerdict extends ScenarioCell {
  readonly status: "passed" | "failed" | "skipped" | "unavailable" | "incomplete";
  readonly cleanup: "passed" | "failed" | "pending";
  readonly evidenceDigest: string;
}

const BOTH_ENTRY_POINTS = ["cli", "sdk"] as const;
const BOTH_LAYERS = {
  e2e: BOTH_ENTRY_POINTS,
  user: BOTH_ENTRY_POINTS
} as const;

/**
 * Public journey ownership for the shared execution-runtime parity gate. These records
 * describe required evidence, not a reason to treat an existing source file as
 * proof for every generated cell. The release aggregator still requires one
 * explicit verdict for each layer/entry-point/runtime cell.
 */
export const PUBLIC_RUNTIME_PARITY_SCENARIOS: readonly PublicRuntimeParityScenario[] = [
  {
    id: "public.admission-and-identity",
    requirement: "Capability discovery, explicit runtime/size selection, truthful identity, and no fallback",
    runtimes: PARITY_RUNTIME_KINDS,
    layers: BOTH_LAYERS,
    sourceFiles: [
      "test/live/edge-runtime-size.user.test.ts",
      "test/live/edge-session-limits.user.test.ts",
      "test/offline/runtime-and-providers.test.ts"
    ]
  },
  {
    id: "public.cohort-continuity",
    requirement: "Pinned sessions survive deploy, park/follow-up, replay, settle, download, and delete",
    runtimes: PARITY_RUNTIME_KINDS,
    layers: BOTH_LAYERS,
    sourceFiles: [
      "test/live/edge-chat-multiturn.user.test.ts",
      "test/live/edge-chat-replay.user.test.ts",
      "test/live/edge-session-lifecycle.user.test.ts"
    ]
  },
  {
    id: "public.controls-and-recovery",
    requirement: "Cancel, suspend/resume, retries, idempotency, host recovery, and honest failure",
    runtimes: PARITY_RUNTIME_KINDS,
    layers: BOTH_LAYERS,
    sourceFiles: [
      "test/live/edge-chat-cancel-launch.user.test.ts",
      "test/live/edge-chat-cancel-send.user.test.ts",
      "test/live/edge-chat-suspend.user.test.ts",
      "test/live/edge-idempotency.user.test.ts"
    ]
  },
  {
    id: "public.conversation",
    requirement: "Single/multi-turn chat, streaming, disconnect, reconnect, and replay",
    runtimes: PARITY_RUNTIME_KINDS,
    layers: BOTH_LAYERS,
    sourceFiles: [
      // live-sdk-chat-session.test.ts was retired 2026-07-27: its two cases were
      // suspend/resume + follow-up + delete, and message idempotency + concurrent
      // busy — already owned by the edge-chat-* shards below and by the platform
      // ledger's live.aws.message-idempotency.v1.
      "test/live/edge-chat-multiturn.user.test.ts",
      "test/live/edge-chat-suspend.user.test.ts",
      "test/live/edge-chat-concurrency.user.test.ts",
      "test/live/edge-chat-replay.user.test.ts",
      "test/live/edge-event-stream.user.test.ts",
      "test/live/live-sdk-event-stream.test.ts"
    ]
  },
  {
    id: "public.files-and-checkpoints",
    requirement: "Checkpoint visibility, byte-identical downloads, metadata, restore, and terminal barrier",
    runtimes: PARITY_RUNTIME_KINDS,
    layers: BOTH_LAYERS,
    sourceFiles: [
      // live-sdk-download-namespaces.test.ts was retired 2026-07-27: edge-files
      // already probes download_files_zip / download_all_zip /
      // download_metadata_zip with the same PK-magic and no-diagnostics-leak
      // checks, and test/offline/download-namespaces.test.ts pins the verb set.
      "test/live/edge-files.user.test.ts",
      "test/offline/download-namespaces.test.ts",
      "test/live/live-sdk-files-and-failures.test.ts"
    ]
  },
  {
    id: "public.limits-and-failures",
    requirement: "Stable errors for validation, provider/tool/MCP failures, timeouts, limits, and runaway",
    runtimes: PARITY_RUNTIME_KINDS,
    layers: BOTH_LAYERS,
    sourceFiles: [
      "test/live/edge-errors-validation.user.test.ts",
      "test/live/edge-session-limits.user.test.ts",
      "test/live/edge-subagent-mcp-failmodes.user.test.ts"
    ]
  },
  {
    id: "public.resolved-behavior",
    requirement: "System, prompt, assets, instructions, packages, environment, networking, secrets, and MCP reach the runtime",
    runtimes: PARITY_RUNTIME_KINDS,
    layers: BOTH_LAYERS,
    sourceFiles: [
      "test/live/config-envvars.user.test.ts",
      "test/live/config-packages.user.test.ts",
      // config-instructions and config-networking were retired 2026-07-27:
      // edge-instructions-files case 5 composes two files PLUS instructions, and
      // edge-mcp-egress proves the same networking:limited allowlist as well as
      // cloud-metadata blocking.
      "test/live/edge-instructions-files.user.test.ts",
      "test/live/edge-mcp-egress.user.test.ts",
      // Secrets reach the runtime: the BYOK-named suite was deleted with the
      // managed-gateway pivot (2026-07-24). Workspace/env secrets are a separate,
      // live feature, and this suite is what covers them now.
      "test/live/live-sdk-tool-capability-fuzz.test.ts"
    ]
  },
  {
    id: "public.retention-and-accounting",
    requirement: "Usage, lineage, settle, list/search, deletion, retention, storage accrual, and cleanup",
    runtimes: PARITY_RUNTIME_KINDS,
    layers: BOTH_LAYERS,
    sourceFiles: [
      "test/live/edge-chat-delete.user.test.ts",
      "test/live/edge-delete-retention.user.test.ts",
      "test/live/edge-lineage-observability.user.test.ts",
      "test/live/edge-list-search.user.test.ts",
      "test/live/edge-storage-accrual.user.test.ts"
    ]
  },
  {
    id: "public.tools-and-structured-work",
    requirement: "All tool routes, structured output, hooks, approvals, waits, schedules, and subagents",
    runtimes: PARITY_RUNTIME_KINDS,
    layers: BOTH_LAYERS,
    sourceFiles: [
      // postHook rejection asserts `calls === 0` against an injected fetch, so it
      // moved to test/offline/ on 2026-07-27 rather than pay a live runtime arm.
      "test/offline/config-posthook.test.ts",
      "test/live/edge-builtin-web-tools.user.test.ts",
      "test/live/edge-skills-tools.user.test.ts",
      "test/live/live-sdk-builtin-tools.test.ts",
      "test/live/live-sdk-tool-capability-fuzz.test.ts",
      "test/live/live-skill-tool-invocation.test.ts"
    ]
  }
] as const;

export function scenarioCellKey(cell: ScenarioCell): string {
  return `${cell.scenarioId}/${cell.layer}/${cell.entryPoint}/${cell.runtime}`;
}

export function buildExpectedParityCells(
  scenarios: readonly PublicRuntimeParityScenario[]
): readonly ScenarioCell[] {
  const cells: ScenarioCell[] = [];
  for (const scenario of scenarios) {
    for (const layer of ["e2e", "user"] as const) {
      for (const entryPoint of scenario.layers[layer]) {
        for (const runtime of scenario.runtimes) {
          cells.push({ scenarioId: scenario.id, layer, entryPoint, runtime });
        }
      }
    }
  }
  return cells.sort((left, right) => scenarioCellKey(left).localeCompare(scenarioCellKey(right)));
}

export function validateParityVerdicts(
  expected: readonly ScenarioCell[],
  actual: readonly ScenarioVerdict[]
): readonly string[] {
  const failures: string[] = [];
  const expectedKeys = new Set(expected.map(scenarioCellKey));
  const actualByKey = new Map<string, ScenarioVerdict[]>();

  for (const verdict of actual) {
    const key = scenarioCellKey(verdict);
    const existing = actualByKey.get(key) ?? [];
    existing.push(verdict);
    actualByKey.set(key, existing);
    if (!expectedKeys.has(key)) failures.push(`unexpected verdict: ${key}`);
    if (verdict.cleanup !== "passed") {
      failures.push(`cleanup verdict: ${key} cleanup=${verdict.cleanup}`);
    }
    if (!verdict.evidenceDigest.startsWith("sha256:")) {
      failures.push(`invalid evidence digest: ${key}`);
    }
  }

  for (const cell of expected) {
    const key = scenarioCellKey(cell);
    const verdicts = actualByKey.get(key) ?? [];
    if (verdicts.length === 0) {
      failures.push(`missing verdict: ${key}`);
      continue;
    }
    if (verdicts.length > 1) failures.push(`duplicate verdict: ${key}`);
    for (const verdict of verdicts) {
      if (verdict.status !== "passed") {
        failures.push(`non-passing verdict: ${key} status=${verdict.status}`);
      }
    }
  }

  return failures.sort();
}
