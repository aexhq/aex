import type { RouteId } from "@aexhq/sdk";

export type UserSuite = "packed" | "local" | "live" | "browser";

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
  scenario("packed.cli-wire-route-registry", ["cli"]),
  scenario("packed.cli-retired-surface", ["cli"]),
  scenario("packed.cli-config-precedence", ["cli"]),
  scenario("packed.cli-secret-redaction", ["cli"]),
  scenario("packed.cli-exit-codes", ["cli"]),
  scenario("packed.cli-output-discipline", ["cli"]),
  scenario("packed.cli-download-partfile", ["cli"]),
  scenario("packed.cli-sigint", ["cli"]),
  scenario("packed.cli-sdk-wire-compatibility", ["sdk", "cli"]),
  scenario("local.site-determinism", ["site"]),
  scenario("local.site-references", ["site"]),
  scenario("local.site-snippets", ["site", "sdk"]),
  scenario("local.site-gates", ["site"]),
  scenario("local.dashboard-no-db", ["dashboard"]),
  scenario("local.dashboard-budgets", ["dashboard"]),
  scenario("local.dashboard-route-allowlist", ["dashboard", "sdk"], ["session_get", "dashboard_bootstrap_get"]),
  scenario("live.session-round-trip", ["sdk", "cli"], ["session_create", "session_get"], 90),
  scenario("live.provider-model-pair", ["sdk", "cli"], ["session_create"], 30),
  scenario("live.cli-parity", ["sdk", "cli"], ["session_create", "session_get"], 120),
  scenario("live.no-dashboard-availability", ["sdk", "cli"], ["session_get"], 30),
  scenario("live.files-inline-overwrite-download-delete", ["sdk", "cli"], ["registry_files_put", "registry_files_get", "registry_files_download_create", "registry_files_delete"], 90),
  scenario("live.files-url-import-is-latest-and-never-falls-back", ["sdk", "cli"], ["registry_files_put", "registry_files_get", "registry_files_download_create"], 120),
  scenario("live.files-direct-upload-publishes-arbitrary-binary", ["sdk", "cli"], ["upload_create", "upload_complete", "registry_files_get"], 180),
  scenario("live.files-session-mount-freezes-content", ["sdk", "cli"], ["registry_files_put", "session_create", "session_message_send", "session_messages_list"], 240),
  scenario("live.storage-persist-overwrites-current-without-guest-aws-credentials", ["sdk", "cli"], ["session_create", "session_message_send", "session_messages_list", "registry_files_get"], 240),
  scenario("live.file-list", ["sdk"], ["registry_files_list"], 30),
  scenario("live.session-message-stream-structured-output", ["sdk"], ["session_create", "session_message_send", "session_messages_stream"], 240),
  scenario("live.sandbox-disabled-is-a-structured-tool-error", ["sdk"], ["session_create", "session_message_send", "session_messages_list"], 240),
  scenario("live.telemetry-stream-replay-and-compressed-download", ["sdk", "cli"], ["session_create", "session_message_send", "session_messages_list", "session_telemetry_stream", "session_telemetry_replay", "session_telemetry_download_create"], 300),
  scenario("live.native-subagent-create-and-event-driven-wait", ["sdk"], ["session_create", "session_message_send", "session_messages_list"], 300),
  scenario("live.remote-mcp-is-invoked-through-the-sandbox-builtin", ["sdk"], ["session_create", "session_message_send", "session_messages_list"], 300),
  scenario("live.sandbox-mcp-is-invoked-through-the-sandbox-builtin", ["sdk"], ["session_create", "session_message_send", "session_messages_list"], 360),
  scenario("live.seven-official-provider-families", ["sdk"], ["session_create", "session_message_send", "session_messages_list"], 600),
  scenario("live.essential-billing-read-card-and-topup-surfaces", ["sdk"], ["billing_balance_get", "billing_payment_method_session_create", "billing_payment_methods_list", "billing_top_up_checkout_create", "billing_transactions_list", "billing_usage_get"], 180),
  scenario("live.session-termination-retains-history-and-rejects-new-work", ["sdk", "cli"], ["session_create", "session_message_send", "session_messages_list", "session_terminate", "session_get"], 300),
  scenario("live.long-context-rolls-forward-without-a-semantic-turn-cap", ["sdk"], ["session_create", "session_message_send", "session_messages_list"], 900),
  scenario("browser.signin-oauth", ["dashboard"], [], 60),
  scenario("browser.csrf", ["dashboard"], [], 30),
  scenario("browser.authz-negative", ["dashboard"], [], 60),
  scenario("browser.session-lifecycle", ["dashboard"], [], 60),
  scenario("browser.panel-degradation", ["dashboard"], [], 60),
  scenario("browser.error-mapping", ["dashboard"], [], 60),
]);
