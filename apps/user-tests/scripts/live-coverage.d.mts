export type CoverageTier =
  | "runtime-spotcheck"
  | "runtime-matrix"
  | "runtime-agnostic"
  | "on-demand";
export type PublicEntryPoint = "sdk" | "cli";

export interface CoverageEntry {
  readonly tier: CoverageTier;
  readonly entryPoint: PublicEntryPoint;
  readonly reason: string;
}

export const COVERAGE_TIERS: readonly CoverageTier[];
export const LIVE_TEST_COVERAGE: Readonly<Record<string, CoverageEntry>>;
export const ON_DEMAND_FILES: readonly string[];

export function assertCoverageManifest(files: readonly string[]): string[];
export function coverageFor(file: string): CoverageEntry;
