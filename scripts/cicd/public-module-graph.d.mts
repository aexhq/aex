export interface PublicModuleNode {
  readonly id: string;
  readonly name: string;
  readonly dir: string;
  readonly version: string;
  readonly publishable: boolean;
  readonly dependsOn: readonly string[];
}

export interface PublicModuleGraph {
  readonly modules: readonly PublicModuleNode[];
  readonly byId: ReadonlyMap<string, PublicModuleNode>;
  readonly byName: ReadonlyMap<string, PublicModuleNode>;
}

export interface ModuleMatrixEntry {
  readonly id: string;
  readonly name: string;
  readonly dir: string;
  readonly version: string;
}

export interface ModuleMatrix {
  readonly include: readonly ModuleMatrixEntry[];
}

export interface RoutedChanges {
  readonly repoWide: boolean;
  readonly unowned: readonly string[];
  readonly affected: readonly string[];
  readonly closure: readonly string[];
  readonly publish: readonly string[];
}

export interface RoutingResult {
  readonly routingFailed: boolean;
  readonly routingFailureReason?: string;
  readonly repoWide: boolean;
  readonly unowned: readonly string[];
  readonly affected: readonly string[];
  readonly closure: readonly string[];
  readonly publish: readonly string[];
  readonly checksMatrix: ModuleMatrix;
  readonly publishMatrix: ModuleMatrix;
  readonly changedPaths: number;
}

export interface RouterArgs {
  readonly base: string;
  readonly head: string;
  readonly paths?: readonly string[];
  readonly githubOutput: string;
  readonly repoRoot: string;
}

export function readModuleGraph(repoRoot: string): PublicModuleGraph;
export function dependentClosure(graph: PublicModuleGraph, seedIds: Iterable<string>): string[];
export function dependencyClosure(graph: PublicModuleGraph, seedIds: Iterable<string>): string[];
export function classifyPath(graph: PublicModuleGraph, path: string): string | null;
export function routeChanges(graph: PublicModuleGraph, paths: Iterable<string>): RoutedChanges;
export function moduleMatrix(graph: PublicModuleGraph, ids: readonly string[]): ModuleMatrix;
export function changedPathsFromGit(repoRoot: string, base: string, head: string): string[];
export function parseArgs(argv: readonly string[]): RouterArgs;
export function renderOutputs(result: RoutingResult): string;
export function route(options: {
  readonly repoRoot: string;
  readonly base?: string;
  readonly head?: string;
  readonly paths?: readonly string[];
}): RoutingResult;
export function fallbackResult(repoRoot: string, reason: string): RoutingResult;
export function main(argv?: readonly string[]): RoutingResult;
