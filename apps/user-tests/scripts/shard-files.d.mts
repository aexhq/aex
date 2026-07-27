import type { CoverageEntry, CoverageTier, PublicEntryPoint } from "./live-coverage.mjs";

export type { CoverageEntry, CoverageTier, PublicEntryPoint };
export {
  COVERAGE_TIERS,
  LIVE_TEST_COVERAGE,
  ON_DEMAND_FILES,
  assertCoverageManifest,
  coverageFor
} from "./live-coverage.mjs";

export type RuntimeKind = "container" | "spot_container" | "lambda";

export interface ShardBin {
  files: string[];
  seconds: number;
}

export interface FileMatrixEntry {
  shard: number;
  count: number;
  /** `files` joined by a space — the argv string CI forwards to the runner. */
  file: string;
  files: readonly string[];
  runtimeKind: RuntimeKind;
  tier: Exclude<CoverageTier, "on-demand">;
  parityCells: readonly {
    scenarioId: string;
    layer: "user";
    entryPoint: PublicEntryPoint;
    runtime: RuntimeKind;
  }[];
  sessionSlots: number;
}

export interface RuntimeCoverage {
  /** Runtime kinds owing full ledger coverage on the target plane. */
  readonly fullCoverage?: readonly RuntimeKind[];
  /** Bins the runtime-agnostic tier is duration-packed into. */
  readonly agnosticShards?: number;
}

export const LIVE_TEST_SHARD_CONFIG: {
  readonly excludedFiles: readonly string[];
  readonly excludedDirectories: readonly string[];
  readonly sessionSlotOverrides: Readonly<Record<string, number>>;
  readonly defaultAgnosticShards: number;
};

export function collectAllLiveFiles(root?: string): string[];
export function collectTestFiles(root?: string): string[];
export function filesInTier(tier: CoverageTier, files?: readonly string[]): string[];
export function sessionSlotsForFile(file: string): number;
export function declaredPeakSessionSlots(files: readonly string[]): number;
export function loadDurations(path?: string): Map<string, number>;
export function lptPartition(
  files: readonly string[],
  durations: Map<string, number>,
  shardCount: number
): ShardBin[];
export function excludeFiles(files: readonly string[], excludedFiles: readonly string[]): string[];
export function buildFileMatrix(files: readonly string[], coverage?: RuntimeCoverage): FileMatrixEntry[];
