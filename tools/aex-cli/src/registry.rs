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
    /// Whether the contract declares the route and nothing serves it yet.
    ///
    /// Read from the generated table, never listed here. The command stays in
    /// the closed tree and is marked: deleting it and re-adding it as routes
    /// land would churn the tree and five shell completions once per landing,
    /// and would hide from the surface a customer explores first that the
    /// capability exists and is coming.
    pub deferred: bool,
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
        ("session cancel", "session_cancel"),
        ("session suspend", "session_suspend"),
        ("session resume", "session_resume"),
        ("session terminate", "session_terminate"),
        ("session delete", "session_delete"),
        ("message list", "session_messages_list"),
        ("message send", "session_message_send"),
        ("approval list", "session_approvals_list"),
        ("approval get", "session_approval_get"),
        ("approval respond", "session_approval_respond"),
        ("operation list", "regional_operations_list"),
        ("operation get", "regional_operation_get"),
        ("operation cancel", "regional_operation_cancel"),
        ("provider-credential list", "provider_credentials_list"),
        ("provider-credential get", "provider_credential_get"),
        (
            "provider-credential register",
            "provider_credential_register",
        ),
        ("provider-credential revoke", "provider_credential_revoke"),
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
                deferred: descriptor.deferred,
                route: descriptor.id,
            }
        })
        .collect()
}

/// What `--help` renders beside a command whose route is not built yet.
pub const DEFERRED_MARKER: &str = "(not yet available)";

/// The customer command tree, with every deferred-backed leaf marked.
///
/// The mark is applied here rather than authored on the clap declarations
/// because the deferral is the contract's fact, not the CLI's: a command whose
/// route lands loses its mark in the same regeneration that publishes the
/// route, with no edit here. Invoking a marked command still performs the call
/// and surfaces the `501` — the server is the only authority on what it serves,
/// and an installed CLI pins one contract digest forever.
///
/// # Panics
///
/// Panics during startup when a registry path names no command in the tree.
#[must_use]
pub fn marked_command() -> clap::Command {
    let mut command = Cli::command();
    for entry in command_registry() {
        if !entry.deferred {
            continue;
        }
        let path: Vec<&str> = entry.path.split(' ').collect();
        command = mark_deferred(command, &path, entry.path);
    }
    command
}

/// Applies [`DEFERRED_MARKER`] to the leaf `path` names.
fn mark_deferred(command: clap::Command, path: &[&str], full: &str) -> clap::Command {
    let Some((head, rest)) = path.split_first() else {
        return command;
    };
    assert!(
        command.find_subcommand(head).is_some(),
        "command registry path `{full}` names no command in the tree"
    );
    command.mut_subcommand(*head, |subcommand| {
        if rest.is_empty() {
            let about = subcommand
                .get_about()
                .map(ToString::to_string)
                .filter(|text| !text.is_empty())
                .map_or_else(
                    || DEFERRED_MARKER.to_owned(),
                    |text| format!("{text} {DEFERRED_MARKER}"),
                );
            subcommand.about(about)
        } else {
            mark_deferred(subcommand, rest, full)
        }
    })
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
    generate(shell, &mut marked_command(), "aex", &mut output);
    Ok(output)
}
