import type { CanaryManifest, CanaryManifestPackage } from "./canary-manifest.d.mts";

export interface VerifiedCanarySelection {
  readonly name: string;
  readonly module: string;
  readonly version: string;
  readonly integrity: string;
  readonly sourceSha: string;
  readonly sourceTag: string;
  readonly upstream: Record<string, string>;
  readonly currentDistTags: Record<string, string>;
}

export interface VerifyCanaryOptions {
  readonly manifest: unknown;
  readonly module: string;
  readonly version: string;
  readonly distTag: string;
  readonly registry?: string;
}

export interface VerifyCanaryDependencies {
  readonly fetch?: (url: string, init?: unknown) => Promise<{
    readonly ok: boolean;
    readonly status: number;
    json(): Promise<unknown>;
  }>;
}

export function selectManifestEntry(
  manifest: CanaryManifest,
  moduleId: string,
  version: string
): CanaryManifestPackage;
export function verifyCanarySelection(
  options: VerifyCanaryOptions,
  dependencies?: VerifyCanaryDependencies
): Promise<VerifiedCanarySelection>;
export function main(argv?: readonly string[]): Promise<VerifiedCanarySelection>;
