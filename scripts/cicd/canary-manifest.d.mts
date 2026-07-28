export declare const CANARY_MANIFEST_SCHEMA: "aex.public-canary-manifest.v1";

export interface CanaryManifestPackage {
  readonly module: string;
  readonly name: string;
  readonly version: string;
  readonly npmDistTag: "canary";
  readonly integrity: string;
  readonly sourceRepository: string;
  readonly sourceSha: string;
  readonly sourceTag: string;
  readonly upstream: Record<string, string>;
}

export interface CanaryManifest {
  readonly schema: string;
  readonly repository: string;
  readonly sourceSha: string;
  readonly generatedAt: string;
  readonly workflowRunId: string;
  readonly packages: readonly CanaryManifestPackage[];
}

export interface CanaryReleaseEntry {
  readonly module: string;
  readonly name: string;
  readonly version: string;
  readonly integrity: string;
  readonly sourceTag: string;
}

export function parseCanaryManifest(value: unknown): CanaryManifest;
export function buildCanaryManifest(options: {
  readonly repoRoot: string;
  readonly repository: string;
  readonly sourceSha: string;
  readonly workflowRunId: string;
  readonly generatedAt: string;
  readonly entries: readonly CanaryReleaseEntry[];
}): CanaryManifest;
export function readReleaseEntries(paths: readonly string[]): CanaryReleaseEntry[];
export function main(argv?: readonly string[]): CanaryManifest;
