export interface ShardBin {
  files: string[];
  seconds: number;
}

export interface FileMatrixEntry {
  shard: number;
  count: number;
  file: string;
  files: readonly string[];
  sessionSlots: 1;
}

export const LIVE_TEST_SHARD_CONFIG: {
  readonly excludedDirectories: readonly string[];
  readonly defaultShards: number;
};

export function collectTestFiles(root?: string): string[];
export function loadDurations(path?: string): Map<string, number>;
export function lptPartition(
  files: readonly string[],
  durations: Map<string, number>,
  shardCount: number
): ShardBin[];
export function excludeFiles(files: readonly string[], excludedFiles: readonly string[]): string[];
export function buildFileMatrix(
  files: readonly string[],
  options?: { readonly shards?: number; readonly durations?: Map<string, number> }
): FileMatrixEntry[];
