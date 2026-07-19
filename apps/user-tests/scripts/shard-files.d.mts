export interface ShardBin {
  files: string[];
  seconds: number;
}

export interface FileMatrixEntry {
  shard: number;
  count: number;
  file: string;
  runtimeKind: "container" | "spot_container" | "lambda" | null;
  parityCells: readonly {
    scenarioId: string;
    layer: "user";
    entryPoint: "sdk" | "cli";
    runtime: "container" | "spot_container" | "lambda";
  }[];
  sessionSlots: number;
}

export const RUNTIME_PAIRED_FILES: ReadonlySet<string>;

export function collectTestFiles(root?: string): string[];
export function sessionSlotsForFile(file: string): number;
export function declaredPeakSessionSlots(files: string[]): number;
export function loadDurations(path?: string): Map<string, number>;
export function lptPartition(files: string[], durations: Map<string, number>, shardCount: number): ShardBin[];
export function excludeFiles(files: string[], excludedFiles: string[]): string[];
export function buildFileMatrix(
  files: string[],
  runtimeKinds?: readonly ("container" | "spot_container" | "lambda")[]
): FileMatrixEntry[];
