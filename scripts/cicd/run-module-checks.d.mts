export declare const MODULE_LANES: readonly ["build", "typecheck", "lint", "test:unit"];

export interface ModuleLanePlan {
  readonly module: string;
  readonly name: string;
  readonly declared: readonly string[];
  readonly undeclared: readonly string[];
}

export interface ModuleLaneOutcome extends ModuleLanePlan {
  readonly results: readonly { readonly lane: string; readonly status: number }[];
  readonly ok: boolean;
}

export function planModuleLanes(repoRoot: string, moduleId: string): ModuleLanePlan;
export function runModuleLanes(
  repoRoot: string,
  moduleId: string,
  run?: (repoRoot: string, packageName: string, lane: string) => number
): ModuleLaneOutcome;
export function main(argv?: readonly string[]): ModuleLaneOutcome;
