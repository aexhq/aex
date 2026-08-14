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
    /// Whether the route is deferred. Every launch CLI row must be false.
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
        ("session create", "session_create"),
        ("session list", "sessions_list"),
        ("session get", "session_get"),
        ("session terminate", "session_terminate"),
        ("session delete", "session_delete"),
        ("message send", "session_message_send"),
        ("message list", "session_messages_list"),
        ("message tail", "session_messages_stream"),
        ("file put", "registry_files_put"),
        ("file upload", "upload_create"),
        ("file list", "registry_files_list"),
        ("file get", "registry_files_get"),
        ("file download", "registry_files_download_create"),
        ("file delete", "registry_files_delete"),
        ("telemetry tail", "session_telemetry_stream"),
        ("telemetry replay", "session_telemetry_replay"),
        ("telemetry download", "session_telemetry_download_create"),
        ("billing balance", "billing_balance_get"),
        ("billing cards", "billing_payment_methods_list"),
        (
            "billing setup-card",
            "billing_payment_method_session_create",
        ),
        ("billing remove-card", "billing_payment_method_delete"),
        ("billing topup", "billing_top_up_checkout_create"),
        ("billing transactions", "billing_transactions_list"),
        ("billing usage", "billing_usage_get"),
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

/// Compatibility marker retained for consumers of the registry library.
pub const DEFERRED_MARKER: &str = "(not yet available)";

/// The customer command tree.
///
/// The exact launch surface contains no deferred commands. Startup still
/// verifies this against the generated route authority so a dead placeholder
/// cannot become visible after contract drift.
#[must_use]
pub fn marked_command() -> clap::Command {
    let command = Cli::command();
    for entry in command_registry() {
        assert!(
            !entry.deferred,
            "launch CLI command `{}` references a deferred route",
            entry.path
        );
    }
    command
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
