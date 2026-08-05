import type { RouteId } from "@aexhq/sdk";

export type UserSuite = "packed" | "local" | "live" | "browser" | "money" | "operator";

export interface UserScenario {
  readonly id: `${UserSuite}.${string}`;
  readonly suite: UserSuite;
  readonly file: string;
  readonly plane: "none" | "local" | "dev";
  readonly owners: readonly string[];
  readonly routes: readonly RouteId[];
  readonly artifacts: readonly ("sdk" | "cli" | "dashboard" | "site")[];
  readonly proves: readonly string[];
  readonly estimatedSeconds: number;
}

function scenario(
  id: UserScenario["id"],
  artifacts: UserScenario["artifacts"],
  routes: readonly RouteId[] = [],
  estimatedSeconds = 10,
): UserScenario {
  const suite = id.slice(0, id.indexOf(".")) as UserSuite;
  return Object.freeze({
    id,
    suite,
    file: `test/${suite}/registered.test.ts`,
    plane: suite === "packed" ? "none" : suite === "local" ? "local" : "dev",
    owners: Object.freeze(["clients"]),
    routes: Object.freeze(routes),
    artifacts: Object.freeze(artifacts),
    proves: Object.freeze([id]),
    estimatedSeconds,
  });
}

export const USER_SCENARIOS: readonly UserScenario[] = Object.freeze([
  scenario("packed.sdk-install-node", ["sdk"]),
  scenario("packed.sdk-install-bun", ["sdk"]),
  scenario("packed.sdk-no-node-builtins", ["sdk"]),
  scenario("packed.sdk-surface", ["sdk"]),
  scenario("packed.sdk-error-precedence", ["sdk"]),
  scenario("packed.sdk-retry-policy", ["sdk"]),
  scenario("packed.sdk-stream-reconnect", ["sdk"]),
  scenario("packed.sdk-download-verify", ["sdk"]),
  scenario("packed.cli-archive-integrity", ["cli"]),
  scenario("packed.cli-os-arch", ["cli"]),
  scenario("packed.cli-help-golden", ["cli"]),
  scenario("packed.cli-completions", ["cli"]),
  scenario("packed.cli-retired-surface", ["cli"]),
  scenario("packed.cli-config-precedence", ["cli"]),
  scenario("packed.cli-exit-codes", ["cli"]),
  scenario("packed.cli-output-discipline", ["cli"]),
  scenario("packed.cli-download-partfile", ["cli"]),
  scenario("packed.cli-sigint", ["cli"]),
  scenario("packed.cli-auth-device-flow", ["cli"]),
  scenario("local.site-determinism", ["site"]),
  scenario("local.site-references", ["site"]),
  scenario("local.site-snippets", ["site", "sdk"]),
  scenario("local.site-gates", ["site"]),
  scenario("local.dashboard-no-db", ["dashboard"]),
  scenario("local.dashboard-budgets", ["dashboard"]),
  scenario("local.dashboard-route-allowlist", ["dashboard", "sdk"], ["session_get", "workspace_get"]),
  scenario("live.session-round-trip", ["sdk", "cli"], ["session_create", "session_get"], 90),
  scenario("live.provider-model-pair", ["sdk", "cli"], ["session_create"], 30),
  scenario("live.durable-operations", ["sdk", "cli"], [], 120),
  scenario("live.registry-and-uploads", ["sdk", "cli"], [], 90),
  scenario("live.files-and-downloads", ["sdk", "cli"], [], 90),
  scenario("live.telemetry", ["sdk", "cli"], [], 120),
  scenario("live.usage-and-billing", ["sdk", "cli"], [], 90),
  scenario("live.limits-and-errors", ["sdk", "cli"], [], 60),
  scenario("live.cli-parity", ["sdk", "cli"], ["session_create", "session_get"], 120),
  scenario("live.no-dashboard-availability", ["sdk", "cli"], ["session_get"], 30),
  scenario("live.registry-list", ["sdk"], ["registry_files_list"], 30),
  scenario("browser.signin-oauth", ["dashboard"], [], 60),
  scenario("browser.csrf", ["dashboard"], [], 30),
  scenario("browser.authz-negative", ["dashboard"], [], 60),
  scenario("browser.session-lifecycle", ["dashboard"], [], 60),
  scenario("browser.panel-degradation", ["dashboard"], [], 60),
  scenario("browser.error-mapping", ["dashboard"], [], 60),
  scenario("money.top-up-checkout", ["sdk", "cli", "dashboard"], [], 90),
  scenario("money.auto-topup-policy", ["sdk", "cli", "dashboard"], [], 90),
  scenario("money.statement-artifact", ["sdk", "cli", "dashboard"], [], 90),
  scenario("operator.contract-isolation", ["sdk", "cli"], [], 60),
  scenario("operator.audit-visibility", ["dashboard"], [], 60),
]);
