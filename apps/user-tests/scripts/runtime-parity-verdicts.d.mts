export interface ParityCell {
  readonly scenarioId: string;
  readonly layer: "user";
  readonly entryPoint: "sdk" | "cli";
  readonly runtime: "container" | "spot_container" | "lambda";
}

export interface ParityVerdict extends ParityCell {
  readonly status: "passed" | "failed";
  readonly cleanup: "passed" | "pending";
  readonly evidenceDigest: string;
  readonly candidateIdentity: string;
}

export const PARITY_SCENARIO_OWNERSHIP: Readonly<Record<string, {
  readonly layer: "user";
  readonly entryPoint: "sdk" | "cli";
  readonly scenarioIds: readonly string[];
}>>;

export function parityCellsForFile(file: string, runtimeKind: ParityCell["runtime"] | null): ParityCell[];
export function emitVerdicts(options: {
  cells: readonly ParityCell[];
  stepOutcome: string;
  candidateIdentity: string;
  reportPath: string;
  outputPath: string;
}): ParityVerdict[];
export function validateVerdictDirectory(options: {
  matrix: readonly { readonly parityCells?: readonly ParityCell[] }[];
  verdictDirectory: string;
  candidateIdentity: string;
}): { expected: number; actual: number };
