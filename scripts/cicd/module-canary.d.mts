import type { PublicModuleNode } from "./public-module-graph.d.mts";

export interface AppliedModuleCanary {
  readonly module: string;
  readonly name: string;
  readonly version: string;
  readonly sourceSha: string;
}

export function moduleNode(repoRoot: string, moduleId: string): PublicModuleNode;
export function resolveModuleCanaryVersion(repoRoot: string, moduleId: string, sha: string): string;
export function moduleSourceTag(moduleId: string, version: string): string;
export function applyModuleCanary(
  repoRoot: string,
  moduleId: string,
  options: { readonly version: string; readonly sha: string }
): AppliedModuleCanary;
export function moduleUpstreamVersions(repoRoot: string, moduleId: string): Record<string, string>;
export function main(argv?: readonly string[]): unknown;
