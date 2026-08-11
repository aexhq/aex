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
        "approval",
        "operation",
        "file",
        "registry",
        "upload",
        "provider-credential",
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
        "run",
        "secret",
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

/// Help marks exactly the commands whose route the contract defers.
///
/// Both directions matter. A missing mark is the silent failure this exists to
/// remove — `aex session create` would look like any other command and answer
/// `501`. A mark on a command that works is the same defect wearing the other
/// face, and it is what a hand-maintained list produces the week after a route
/// lands.
#[test]
fn help_marks_exactly_the_deferred_backed_commands() {
    let command = aex_cli::marked_command();
    for entry in command_registry() {
        let mut current = &command;
        let mut leaf = None;
        for segment in entry.path.split(' ') {
            let found = current
                .find_subcommand(segment)
                .unwrap_or_else(|| panic!("`{}` names no command", entry.path));
            current = found;
            leaf = Some(found);
        }
        let about = leaf
            .expect("every registry path has at least one segment")
            .get_about()
            .map(ToString::to_string)
            .unwrap_or_default();
        assert_eq!(
            about.contains(aex_cli::DEFERRED_MARKER),
            entry.deferred,
            "`aex {}` renders `{about}` for deferred={}",
            entry.path,
            entry.deferred
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
