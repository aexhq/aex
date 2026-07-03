export interface ShardBin {
  files: string[];
  seconds: number;
}

export function collectTestFiles(root?: string): string[];
export function loadDurations(path?: string): Map<string, number>;
export function lptPartition(files: string[], durations: Map<string, number>, shardCount: number): ShardBin[];
