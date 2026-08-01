//! Native `aex` command-line client.

/// Closed clap command tree.
pub mod cli;
/// Local profile and precedence policy.
pub mod config;
/// Atomic part-file download planning.
pub mod download;
/// Stable process exit classification.
pub mod error;
/// Terminal and machine output policy.
pub mod output;
/// Declarative command-to-route registry.
pub mod registry;

pub use cli::{Cli, Command, CompletionShell};
pub use registry::{CommandRegistryEntry, command_registry, render_completions};
