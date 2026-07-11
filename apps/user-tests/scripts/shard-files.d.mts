export interface ShardBin {
  files: string[];
  seconds: number;
}

export interface FileMatrixEntry {
  shard: number;
  count: number;
  file: string;
  sessionSlots: number;
}

export function collectTestFiles(root?: string): string[];
export function sessionSlotsForFile(file: string): number;
export function declaredPeakSessionSlots(files: string[]): number;
export function loadDurations(path?: string): Map<string, number>;
export function lptPartition(files: string[], durations: Map<string, number>, shardCount: number): ShardBin[];
export function excludeFiles(files: string[], excludedFiles: string[]): string[];
export function buildFileMatrix(files: string[]): FileMatrixEntry[];
