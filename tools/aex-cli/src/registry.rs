use aex_wire::routes::{ROUTES, RouteId};
use clap::CommandFactory;
use clap_complete::{Shell, generate};
use serde::Serialize;

use crate::Cli;

/// One route-backed leaf in the stable command registry.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandRegistryEntry {
    /// Space-delimited command path below `aex`.
    pub path: &'static str,
    /// Generated wire operation id.
    pub route_id: &'static str,
    /// Generated identity used by dispatch; omitted from the JSON view because
    /// `route_id` is its stable wire spelling.
    #[serde(skip)]
    pub route: RouteId,
}

/// The customer command-to-route map.
///
/// # Panics
///
/// Panics during startup when a checked-in command names no generated route.
#[must_use]
pub fn command_registry() -> Vec<CommandRegistryEntry> {
    const ENTRIES: &[(&str, &str)] = &[
        ("account get", "account_get"),
        ("org list", "organizations_list"),
        ("org create", "organization_create"),
        ("org get", "organization_get"),
        ("org member list", "memberships_list"),
        ("org invite create", "invitation_create"),
        ("workspace list", "workspaces_list"),
        ("workspace create", "workspace_create"),
        ("workspace get", "workspace_get"),
        ("workspace delete", "workspace_delete"),
        ("workspace current", "workspace_current_get"),
        ("key list", "api_keys_list"),
        ("key create", "api_key_create"),
        ("key revoke", "api_key_revoke"),
        ("session create", "session_create"),
        ("session list", "sessions_list"),
        ("session get", "session_get"),
        ("session stop", "session_stop"),
        ("session persist", "session_persist"),
        ("session clone", "session_clone"),
        ("session trash", "session_trash"),
        ("session restore", "session_restore"),
        ("session purge", "session_purge"),
        ("session discard", "session_workspace_discard"),
        ("session rebind", "session_credential_rebind"),
        ("message list", "session_messages_list"),
        ("message send", "session_message_send"),
        ("run list", "session_runs_list"),
        ("run get", "session_run_get"),
        ("approval list", "session_approvals_list"),
        ("approval get", "session_approval_get"),
        ("approval respond", "session_approval_respond"),
        ("operation list", "regional_operations_list"),
        ("operation get", "regional_operation_get"),
        ("operation cancel", "regional_operation_cancel"),
        ("secret list", "secrets_list"),
        ("secret get", "secret_get"),
        ("secret set", "secret_put"),
        ("secret delete", "secret_delete"),
        ("secret revoke", "secret_revoke"),
        ("limit list", "workspace_limits_list"),
        ("limit get", "workspace_limit_get"),
        ("billing balance", "billing_balance_get"),
        ("billing portal", "billing_portal_session_create"),
        ("billing topup", "billing_top_up_checkout_create"),
        ("usage query", "usage_query"),
    ];
    ENTRIES
        .iter()
        .map(|(path, route_id)| {
            let Some(descriptor) = ROUTES.iter().find(|route| route.operation_id == *route_id)
            else {
                panic!("command registry references unknown route {route_id}");
            };
            CommandRegistryEntry {
                path,
                route_id,
                route: descriptor.id,
            }
        })
        .collect()
}

/// Render one of the five supported completion formats.
///
/// # Errors
///
/// Returns an error when `shell` is not one of the five closed values.
pub fn render_completions(shell: &str) -> Result<Vec<u8>, String> {
    let shell = match shell {
        "bash" => Shell::Bash,
        "zsh" => Shell::Zsh,
        "fish" => Shell::Fish,
        "powershell" => Shell::PowerShell,
        "elvish" => Shell::Elvish,
        other => return Err(format!("unsupported completion shell {other}")),
    };
    let mut output = Vec::new();
    generate(shell, &mut Cli::command(), "aex", &mut output);
    Ok(output)
}
