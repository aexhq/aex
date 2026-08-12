export const DASHBOARD_BUILD_STEPS: readonly [
  readonly ["run", "--filter", "@aexhq/wire", "build"],
  readonly ["run", "--filter", "@aexhq/sdk", "build"],
  readonly ["run", "vercel", "build", "--standalone", "--no-color"],
];

export function buildDashboardOutput(): Promise<void>;
