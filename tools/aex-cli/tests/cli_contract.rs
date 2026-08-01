//! Public CLI-tree and completion contract tests.

use std::collections::BTreeSet;

use aex_cli::{Cli, command_registry, render_completions};
use clap::CommandFactory;

#[test]
fn clap_tree_is_closed_and_valid() {
    Cli::command().debug_assert();
    let command = Cli::command();
    assert_eq!(command.get_name(), "aex");
    let names = command
        .get_subcommands()
        .map(clap::Command::get_name)
        .collect::<BTreeSet<_>>();
    for expected in [
        "auth",
        "account",
        "org",
        "workspace",
        "key",
        "session",
        "message",
        "run",
        "approval",
        "operation",
        "file",
        "registry",
        "upload",
        "secret",
        "limit",
        "observe",
        "telemetry",
        "usage",
        "billing",
        "completions",
        "config",
        "version",
    ] {
        assert!(names.contains(expected), "missing command group {expected}");
    }
    for retired in [
        "start",
        "chat",
        "proxy",
        "checkpoint",
        "suspend",
        "resume",
        "webhooks",
        "tail",
        "otel",
        "inspect",
        "agents",
        "self-update",
    ] {
        assert!(
            !names.contains(retired),
            "retired command {retired} returned"
        );
    }
}

#[test]
fn command_registry_is_deterministic_and_route_backed() {
    let first = serde_json::to_vec(&command_registry()).expect("registry serializes");
    let second = serde_json::to_vec(&command_registry()).expect("registry serializes twice");
    assert_eq!(first, second);

    let registry = command_registry();
    assert!(
        registry.len() >= 40,
        "customer command registry is unexpectedly small"
    );
    let mut routes = BTreeSet::new();
    for entry in registry {
        assert_eq!(
            aex_wire::routes::route(entry.route).operation_id,
            entry.route_id
        );
        assert!(
            routes.insert(entry.route_id),
            "route mapped twice: {}",
            entry.route_id
        );
    }
}

#[test]
fn five_completion_formats_are_non_empty_and_stable() {
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let first = render_completions(shell).expect("known shell");
        let second = render_completions(shell).expect("known shell twice");
        assert_eq!(first, second);
        assert!(first.len() > 100, "{shell} completion is empty");
    }
}
