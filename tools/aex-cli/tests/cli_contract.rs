//! Public CLI-tree, route registry, help, and completion contract tests.

use std::collections::BTreeSet;

use aex_cli::{Cli, command_registry, render_completions};
use clap::{CommandFactory as _, Parser as _};

#[test]
fn clap_tree_is_the_exact_session_centered_surface() {
    Cli::command().debug_assert();
    let command = Cli::command();
    assert_eq!(command.get_name(), "aex");
    assert_eq!(
        names(&command),
        set([
            "account",
            "api-key",
            "session",
            "message",
            "file",
            "telemetry",
            "billing",
            "config",
            "version"
        ])
    );
    assert_eq!(
        subcommands(&command, "account"),
        set(["bootstrap", "create"])
    );
    assert_eq!(
        subcommands(&command, "api-key"),
        set(["create", "list", "revoke"])
    );
    assert_eq!(
        subcommands(&command, "session"),
        set(["create", "list", "get", "terminate", "delete"])
    );
    assert_eq!(
        subcommands(&command, "message"),
        set(["send", "list", "tail"])
    );
    assert_eq!(
        subcommands(&command, "file"),
        set(["put", "upload", "list", "get", "download", "delete"])
    );
    assert_eq!(
        subcommands(&command, "telemetry"),
        set(["tail", "replay", "download"])
    );
    assert_eq!(
        subcommands(&command, "billing"),
        set([
            "balance",
            "cards",
            "setup-card",
            "remove-card",
            "topup",
            "transactions",
            "usage"
        ])
    );
}

#[test]
fn removed_product_nouns_and_secret_arguments_are_absent_from_help() {
    let mut help = Vec::new();
    Cli::command().write_long_help(&mut help).expect("help");
    let help = String::from_utf8(help).expect("UTF-8 help");
    for removed in [
        "auth",
        "org",
        "approval",
        "operation",
        concat!("provider", "-credential"),
        "limit",
        "observe",
        "usage query",
        "completions",
        "device",
        concat!("account", " token"),
        "--api-key",
        "--provider-key ",
        "--mcp-secret",
    ] {
        assert!(
            !help.contains(removed),
            "removed surface returned in help: {removed}\n{help}"
        );
    }
}

#[test]
fn account_and_api_key_arguments_are_minimal_and_scope_is_repeatable() {
    assert!(Cli::try_parse_from(["aex", "account", "bootstrap"]).is_ok());
    assert!(
        Cli::try_parse_from(["aex", "account", "create", "--name", "first-workspace-key",]).is_ok()
    );
    assert!(Cli::try_parse_from(["aex", "account", "create"]).is_err());
    assert!(
        Cli::try_parse_from([
            "aex",
            "api-key",
            "create",
            "--name",
            "automation",
            "--scope",
            "sessions:read",
            "--scope",
            "resources:write",
        ])
        .is_ok()
    );
    assert!(
        Cli::try_parse_from(["aex", "api-key", "create", "--name", "automation"]).is_ok(),
        "omitting --scope selects the workspace-key defaults at execution"
    );
    assert!(
        Cli::try_parse_from([
            "aex",
            "api-key",
            "create",
            "--name",
            "automation",
            "--scope",
            "billing:write",
        ])
        .is_err(),
        "dashboard-session scopes are not workspace-key scopes"
    );
    assert!(Cli::try_parse_from(["aex", "api-key", "list"]).is_ok());
    assert!(
        Cli::try_parse_from(["aex", "api-key", "revoke", "key_0000000000e0081040g2081040",])
            .is_ok()
    );
}

#[test]
fn provider_and_mcp_secrets_have_only_env_or_stdin_sources() {
    assert!(
        Cli::try_parse_from([
            "aex",
            "session",
            "create",
            "--provider",
            "openai",
            "--model",
            "gpt",
            "--provider-key-env",
            "OPENAI_API_KEY",
        ])
        .is_ok()
    );
    assert!(
        Cli::try_parse_from([
            "aex",
            "session",
            "create",
            "--provider",
            "openai",
            "--model",
            "gpt",
            "--provider-key-stdin",
            "--mcp-json-env",
            "AEX_MCP_JSON",
        ])
        .is_ok()
    );
    assert!(
        Cli::try_parse_from([
            "aex",
            "session",
            "create",
            "--provider",
            "openai",
            "--model",
            "gpt",
        ])
        .is_err()
    );
    assert!(
        Cli::try_parse_from([
            "aex",
            "session",
            "create",
            "--provider",
            "openai",
            "--model",
            "gpt",
            "--provider-key",
            "visible-secret",
        ])
        .is_err()
    );
}

#[test]
fn command_registry_is_exact_deterministic_route_backed_and_live() {
    let first = serde_json::to_vec(&command_registry()).expect("registry serializes");
    let second = serde_json::to_vec(&command_registry()).expect("registry serializes twice");
    assert_eq!(first, second);

    let registry = command_registry();
    assert_eq!(registry.len(), 29);
    let mut routes = BTreeSet::new();
    for entry in registry {
        assert_eq!(
            aex_wire::routes::route(entry.route).operation_id,
            entry.route_id
        );
        assert!(
            !entry.deferred,
            "visible command is a dead placeholder: {}",
            entry.path
        );
        assert!(
            routes.insert(entry.route_id),
            "route mapped twice: {}",
            entry.route_id
        );
    }
}

#[test]
fn five_completion_formats_are_non_empty_stable_and_contain_no_removed_noun() {
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let first = render_completions(shell).expect("known shell");
        let second = render_completions(shell).expect("known shell twice");
        assert_eq!(first, second);
        assert!(first.len() > 100, "{shell} completion is empty");
        let text = String::from_utf8_lossy(&first);
        assert!(!text.contains(concat!("provider", "-credential")));
        assert!(!text.contains("--api-key"));
    }
}

fn names(command: &clap::Command) -> BTreeSet<&str> {
    command
        .get_subcommands()
        .map(clap::Command::get_name)
        .collect()
}

fn subcommands<'a>(command: &'a clap::Command, name: &str) -> BTreeSet<&'a str> {
    names(command.find_subcommand(name).expect("group exists"))
}

fn set<const N: usize>(values: [&'static str; N]) -> BTreeSet<&'static str> {
    values.into_iter().collect()
}
