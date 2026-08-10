export const ACTIONS = ["fmt", "build", "check", "clippy", "test"] as const;
export type Action = (typeof ACTIONS)[number];

export interface Arguments {
  readonly actions: Action[];
  readonly base: string;
  readonly help: boolean;
  readonly jobs: number;
  readonly releaseTool?: string;
  readonly targetDir?: string;
}

interface SelectedNode {
  readonly id: string;
  readonly reason: string;
  readonly path?: string;
  readonly of?: string;
}

export interface SelectionDocument {
  readonly schema: "aex.selection.v1";
  readonly lane: string;
  readonly mode: string;
  readonly test: readonly SelectedNode[];
  readonly deploy: readonly SelectedNode[];
  readonly scenarios: readonly SelectedNode[];
  readonly repo_wide: boolean;
  readonly unowned: readonly string[];
  readonly changed_paths: number;
  readonly routing_failed: boolean;
  readonly routing_failure_reason: string | null;
}

export interface CommandPlan {
  readonly program: string;
  readonly args: readonly string[];
}

export interface AffectedPlan {
  readonly changedPaths: number;
  readonly commands: readonly CommandPlan[];
  readonly excludedEvidence: readonly string[];
  readonly nodePackages: readonly string[];
  readonly repoWide: boolean;
  readonly rustPackages: readonly string[];
  readonly scenarios: number;
}

const FORMAT_BATCH_SIZE = 16;

export function parseArguments(argv: readonly string[], defaultJobs: number): Arguments {
  const parsed: {
    actions: Action[];
    base: string;
    help: boolean;
    jobs: number;
    releaseTool?: string;
    targetDir?: string;
  } = {
    actions: [],
    base: "main",
    help: false,
    jobs: defaultJobs
  };

  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    if (flag === "--help" || flag === "-h") {
      parsed.help = true;
      continue;
    }
    const value = argv[index + 1];
    if (value === undefined) throw new Error(`${flag} requires a value`);
    index += 1;
    switch (flag) {
      case "--run":
        parsed.actions = normalizeActions(value);
        break;
      case "--base":
        parsed.base = requireText(flag, value);
        break;
      case "--jobs": {
        const jobs = Number(value);
        if (!Number.isSafeInteger(jobs) || jobs < 1) {
          throw new Error("--jobs must be a positive integer");
        }
        parsed.jobs = jobs;
        break;
      }
      case "--target-dir":
        parsed.targetDir = requireText(flag, value);
        break;
      case "--release-tool":
        parsed.releaseTool = requireText(flag, value);
        break;
      default:
        throw new Error(`unknown argument: ${flag}`);
    }
  }
  return parsed;
}

function normalizeActions(value: string): Action[] {
  const requested = value === "all" ? [...ACTIONS] : value.split(",");
  for (const action of requested) {
    if (!ACTIONS.includes(action as Action)) throw new Error(`unknown action: ${action}`);
  }
  const selected = new Set(requested as Action[]);
  return ACTIONS.filter((action) => selected.has(action));
}

function requireText(flag: string, value: string): string {
  if (value.trim().length === 0) throw new Error(`${flag} requires a non-empty value`);
  return value;
}

export function parseNullSeparated(input: string): string[] {
  return input.split("\0").filter((path) => path.length > 0);
}

export function mergeChangedPaths(...groups: readonly (readonly string[])[]): string[] {
  return [
    ...new Set(
      groups.flatMap((group) => group).map((path) => path.replaceAll("\\", "/"))
    )
  ].sort();
}

export function buildAffectedPlan(
  selection: SelectionDocument,
  actions: readonly Action[],
  jobs: number
): AffectedPlan {
  if (selection.schema !== "aex.selection.v1") {
    throw new Error(`unsupported selection schema: ${selection.schema}`);
  }
  if (selection.routing_failed) {
    throw new Error(`affected routing failed: ${selection.routing_failure_reason ?? "unknown"}`);
  }
  if (selection.unowned.length > 0) {
    throw new Error(`affected routing found unowned paths: ${selection.unowned.join(", ")}`);
  }

  const rustPackages = selectedNames(selection.test, "cargo:");
  const nodePackages = selectedNames(selection.test, "npm:");
  const directRustPackages = selection.repo_wide || selection.mode === "full"
    ? rustPackages
    : selectedNames(
        selection.test.filter((selected) => selected.reason === "changed-source"),
        "cargo:"
      );
  const commands: CommandPlan[] = [];

  for (const action of ACTIONS) {
    if (!actions.includes(action)) continue;
    if (action === "fmt") {
      for (const packages of batches(directRustPackages, FORMAT_BATCH_SIZE)) {
        commands.push(cargo("fmt", packages, [], ["--", "--check"]));
      }
    } else if (action === "build") {
      if (rustPackages.length > 0) {
        commands.push(cargo("build", rustPackages, ["--locked", "--jobs", `${jobs}`]));
      }
      addNodeCommand(commands, nodePackages, "build");
    } else if (action === "check") {
      if (rustPackages.length > 0) {
        commands.push(
          cargo("check", rustPackages, ["--locked", "--all-targets", "--jobs", `${jobs}`])
        );
      }
      addNodeCommand(commands, nodePackages, "typecheck");
      addNodeCommand(commands, nodePackages, "lint");
    } else if (action === "clippy") {
      if (rustPackages.length > 0) {
        commands.push(
          cargo(
            "clippy",
            rustPackages,
            ["--locked", "--all-targets", "--jobs", `${jobs}`],
            ["--", "-D", "warnings"]
          )
        );
      }
    } else {
      if (rustPackages.length > 0) {
        commands.push(
          cargo(
            "nextest",
            rustPackages,
            [
              "run",
              "--locked",
              "--profile",
              "default",
              "--build-jobs",
              `${jobs}`,
              "--test-threads",
              `${jobs}`
            ],
            ["--no-tests=fail"]
          )
        );
      }
      addNodeCommand(commands, nodePackages, "test:unit");
    }
  }

  return {
    changedPaths: selection.changed_paths,
    commands,
    excludedEvidence: [
      "doctests and no-skip inventory receipts",
      "integration-engine tests",
      "live, load, and scenario suites",
      "Terraform, artifact, and repository-wide gates",
      "hosted release evidence"
    ],
    nodePackages,
    repoWide: selection.repo_wide || selection.mode === "full",
    rustPackages,
    scenarios: selection.scenarios.length
  };
}

export function assertExecutionAllowed(plan: AffectedPlan, branch: string): void {
  if (plan.repoWide && branch !== "main") {
    throw new Error(
      "the existing release graph widened this change to the full workspace; merge the " +
        "focused work into local main and run broad validation there"
    );
  }
}

function selectedNames(nodes: readonly SelectedNode[], prefix: string): string[] {
  return nodes
    .map((selected) => selected.id)
    .filter((id) => id.startsWith(prefix))
    .map((id) => id.slice(prefix.length))
    .sort();
}

function cargo(
  subcommand: string,
  packages: readonly string[],
  options: readonly string[],
  trailing: readonly string[] = []
): CommandPlan {
  return {
    program: "cargo",
    args: [
      subcommand,
      ...options,
      ...packages.flatMap((packageName) => ["-p", packageName]),
      ...trailing
    ]
  };
}

function addNodeCommand(
  commands: CommandPlan[],
  packages: readonly string[],
  script: string
): void {
  if (packages.length === 0) return;
  commands.push({
    program: "bun",
    args: [
      "run",
      ...packages.flatMap((packageName) => ["--filter", packageName]),
      "--parallel",
      "--if-present",
      script
    ]
  });
}

function batches<T>(values: readonly T[], size: number): T[][] {
  const result: T[][] = [];
  for (let start = 0; start < values.length; start += size) {
    result.push(values.slice(start, start + size));
  }
  return result;
}
