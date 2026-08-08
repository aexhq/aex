//! Runs the structural rules, the derived test registry and the flake scanner.
//!
//! Exit code 0 means every rule holds. Any other exit code means a rule failed
//! or the check itself could not run; neither is ever reported as success, and
//! there is no `--force`.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use aex_workspace_check::flake::{
    DoctestSummary, FlakeInput, JUnitReport, NextestList, Rerun, profile_retries,
};
use aex_workspace_check::registry::{PackageRow, Phase};
use clap::{Parser, Subcommand};

/// The workspace structural and test-ownership checker.
#[derive(Debug, Parser)]
#[command(name = "aex-workspace-check", version, about, long_about = None)]
struct Cli {
    /// Which phase to evaluate. `candidate` turns every unearned-evidence row
    /// into a failure.
    #[arg(long, value_enum, default_value = "source-rewrite", global = true)]
    phase: PhaseArgument,
    #[command(subcommand)]
    command: Option<Action>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum PhaseArgument {
    /// Source is being written; unearned evidence is recorded, not failed.
    SourceRewrite,
    /// A release candidate exists; unearned evidence blocks.
    Candidate,
}

impl From<PhaseArgument> for Phase {
    fn from(value: PhaseArgument) -> Self {
        match value {
            PhaseArgument::SourceRewrite => Self::SourceRewrite,
            PhaseArgument::Candidate => Self::Candidate,
        }
    }
}

#[derive(Debug, Subcommand)]
enum Action {
    /// Run every structural and registry rule. The default.
    Check,
    /// Build or verify the derived test registry.
    Registry {
        #[command(subcommand)]
        action: RegistryAction,
    },
    /// Project every member's `//!` header into `[package] description`.
    Description {
        #[command(subcommand)]
        action: DescriptionAction,
    },
    /// Scan one lane's output for skips, empty selections and retries.
    Flake {
        #[command(subcommand)]
        action: Box<FlakeAction>,
    },
}

#[derive(Debug, Subcommand)]
enum DescriptionAction {
    /// Rewrite every member's `[package] description` from its `//!` header.
    Build,
    /// Fail on any member whose description is missing or has drifted.
    ///
    /// The same rule rides the default `check`, so this exists for a focused
    /// run rather than as the gate.
    Verify,
}

#[derive(Debug, Subcommand)]
enum RegistryAction {
    /// Write `release/test-registry.json` and `release/unearned-evidence.json`.
    Build {
        /// `cargo nextest list --message-format json` output, when available.
        #[arg(long)]
        list: Option<PathBuf>,
    },
    /// Recompute both documents and fail on any difference.
    Verify {
        /// `cargo nextest list --message-format json` output, when available.
        #[arg(long)]
        list: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum FlakeAction {
    /// Produce the receipt's `inventory` block and the source-side verdict.
    Scan {
        /// `cargo nextest list --message-format json` output.
        #[arg(long)]
        list: PathBuf,
        /// The lane's `JUnit` report.
        #[arg(long)]
        junit: PathBuf,
        /// `cargo test --doc` output.
        #[arg(long)]
        doctest: Option<PathBuf>,
        /// The lane's nextest profile.
        #[arg(long)]
        profile: String,
        /// The packages the lane selected. Repeat the flag once per package.
        /// Omitting it scopes the scan to the whole workspace, which is only
        /// correct for a lane that actually ran the whole workspace.
        #[arg(long = "package")]
        packages: Vec<String>,
        /// The nextest configuration the lane ran under.
        #[arg(long, default_value = ".config/nextest.toml")]
        nextest_config: PathBuf,
        /// The lane's filter expression, when it passed one.
        #[arg(long)]
        filter: Option<String>,
        /// Exclude targets Cargo cannot build without explicitly selected features.
        #[arg(long)]
        default_features_only: bool,
        /// The receipt this run is a diagnostic rerun of.
        #[arg(long)]
        rerun_of: Option<String>,
        /// The preserved first-failure record of that receipt.
        #[arg(long)]
        first_failure: Option<PathBuf>,
    },
}

const REGISTRY_PATH: &str = "release/test-registry.json";
const UNEARNED_PATH: &str = "release/unearned-evidence.json";

fn main() -> ExitCode {
    let cli = Cli::parse();
    let phase = Phase::from(cli.phase);
    match cli.command.unwrap_or(Action::Check) {
        Action::Check => run_check(phase),
        Action::Registry {
            action: RegistryAction::Build { list },
        } => run_registry(phase, list.as_deref(), true),
        Action::Registry {
            action: RegistryAction::Verify { list },
        } => run_registry(phase, list.as_deref(), false),
        Action::Description {
            action: DescriptionAction::Build,
        } => run_description(true),
        Action::Description {
            action: DescriptionAction::Verify,
        } => run_description(false),
        Action::Flake { action } => run_flake(&action),
    }
}

fn run_description(write: bool) -> ExitCode {
    let json = match metadata_json() {
        Ok(json) => json,
        Err(error) => return fail(&error),
    };
    let metadata = match aex_workspace_check::WorkspaceMetadata::parse(&json) {
        Ok(metadata) => metadata,
        Err(error) => return fail(&error.to_string()),
    };
    let root = PathBuf::from(&metadata.workspace_root);
    let members = match aex_workspace_check::description::read_members(&root, &metadata) {
        Ok(members) => members,
        Err(error) => return fail(&error.to_string()),
    };

    if write {
        // A member whose paragraph is not one sentence is rejected before
        // anything is written: writing it would produce a description the
        // verify half then fails on, which is a tool that disagrees with
        // itself.
        let unusable: Vec<_> = members
            .iter()
            .filter(|member| {
                member.header.is_empty()
                    || aex_workspace_check::description::sentence_count(&member.header) != 1
            })
            .collect();
        if !unusable.is_empty() {
            eprintln!(
                "aex-workspace-check: {} member(s) cannot be described",
                unusable.len()
            );
            for member in unusable {
                eprintln!(
                    "  `{}` — split the opening `//!` paragraph so it is one sentence",
                    member.name
                );
            }
            return ExitCode::FAILURE;
        }
        return match aex_workspace_check::description::build(&root, &members) {
            Ok(changed) => {
                println!("aex-workspace-check: wrote {changed} `[package] description` field(s)");
                ExitCode::SUCCESS
            }
            Err(error) => fail(&error.to_string()),
        };
    }

    let violations = aex_workspace_check::description::check(&members);
    if violations.is_empty() {
        println!(
            "aex-workspace-check: all {} member description(s) match their `//!` header",
            members.len()
        );
        return ExitCode::SUCCESS;
    }
    eprintln!("aex-workspace-check: {} violation(s)", violations.len());
    for violation in &violations {
        eprintln!("  {violation}");
    }
    ExitCode::FAILURE
}

fn metadata_json() -> Result<String, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = Command::new(&cargo)
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output()
        .map_err(|error| format!("cannot run `{cargo} metadata`: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "`{cargo} metadata` failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn run_check(phase: Phase) -> ExitCode {
    let json = match metadata_json() {
        Ok(json) => json,
        Err(error) => return fail(&error),
    };
    let report = match aex_workspace_check::check_workspace(&json, phase) {
        Ok(report) => report,
        Err(error) => return fail(&error.to_string()),
    };
    let violations = report.violations();
    if violations.is_empty() {
        println!(
            "aex-workspace-check: {} package(s) satisfy every structural and registry rule",
            report.collected.packages.len()
        );
        println!(
            "aex-workspace-check: {} unearned-evidence row(s) recorded in the {} phase",
            report.registry.unearned.len(),
            match phase {
                Phase::SourceRewrite => "source-rewrite",
                Phase::Candidate => "candidate",
            }
        );
        return ExitCode::SUCCESS;
    }
    eprintln!("aex-workspace-check: {} violation(s)", violations.len());
    for violation in &violations {
        eprintln!("  {violation}");
    }
    ExitCode::FAILURE
}

fn run_registry(phase: Phase, list: Option<&std::path::Path>, write: bool) -> ExitCode {
    let json = match metadata_json() {
        Ok(json) => json,
        Err(error) => return fail(&error),
    };
    let metadata = match aex_workspace_check::WorkspaceMetadata::parse(&json) {
        Ok(metadata) => metadata,
        Err(error) => return fail(&error.to_string()),
    };
    let root = PathBuf::from(&metadata.workspace_root);
    let collected = match aex_workspace_check::collect::collect(&root, &metadata) {
        Ok(collected) => collected,
        Err(error) => return fail(&error.to_string()),
    };
    let policy = aex_workspace_check::Policy::embedded();
    let mut input = collected.as_input(policy, phase);
    if let Some(path) = list {
        match read_list(path) {
            Ok(counts) => input.collected = Some(counts),
            Err(error) => return fail(&error),
        }
    }
    let document = aex_workspace_check::registry::build(&input);
    let report = aex_workspace_check::registry::check(&input);
    let (Ok(registry_text), Ok(unearned_text)) = (
        serde_json::to_string_pretty(&document),
        serde_json::to_string_pretty(&report.unearned),
    ) else {
        return fail("the registry document could not be rendered");
    };
    let registry_path = root.join(REGISTRY_PATH);
    let unearned_path = root.join(UNEARNED_PATH);

    if write {
        for (path, text) in [
            (&registry_path, &registry_text),
            (&unearned_path, &unearned_text),
        ] {
            if let Err(error) = std::fs::write(path, format!("{text}\n")) {
                return fail(&format!("cannot write `{}`: {error}", path.display()));
            }
        }
        println!("aex-workspace-check: wrote {REGISTRY_PATH} and {UNEARNED_PATH}");
        return ExitCode::SUCCESS;
    }

    let mut stale = Vec::new();
    for (path, text, name) in [
        (&registry_path, &registry_text, REGISTRY_PATH),
        (&unearned_path, &unearned_text, UNEARNED_PATH),
    ] {
        match std::fs::read_to_string(path) {
            Ok(existing) if existing.trim_end() == text.trim_end() => {}
            _ => stale.push(name),
        }
    }
    if stale.is_empty() {
        println!("aex-workspace-check: {REGISTRY_PATH} and {UNEARNED_PATH} are current");
        return ExitCode::SUCCESS;
    }
    eprintln!("aex-workspace-check: 1 violation(s)");
    eprintln!(
        "  [aex-registry-stale] {} differ(s) from what `aex-workspace-check registry build` produces; regenerate rather than hand-merging",
        stale.join(" and ")
    );
    ExitCode::FAILURE
}

fn read_list(path: &std::path::Path) -> Result<std::collections::BTreeMap<String, usize>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read `{}`: {error}", path.display()))?;
    let list = NextestList::parse(&text)
        .map_err(|error| format!("cannot parse `{}`: {error}", path.display()))?;
    Ok(list
        .rust_suites
        .into_iter()
        .map(|(binary, suite)| (binary, suite.testcases.len()))
        .collect())
}

fn run_flake(action: &FlakeAction) -> ExitCode {
    let FlakeAction::Scan {
        list,
        junit,
        doctest,
        profile,
        packages,
        nextest_config,
        filter,
        default_features_only,
        rerun_of,
        first_failure,
    } = action;
    let json = match metadata_json() {
        Ok(json) => json,
        Err(error) => return fail(&error),
    };
    let metadata = match aex_workspace_check::WorkspaceMetadata::parse(&json) {
        Ok(metadata) => metadata,
        Err(error) => return fail(&error.to_string()),
    };
    let root = PathBuf::from(&metadata.workspace_root);
    let collected = match aex_workspace_check::collect::collect(&root, &metadata) {
        Ok(collected) => collected,
        Err(error) => return fail(&error.to_string()),
    };

    let list_text = match std::fs::read_to_string(list) {
        Ok(text) => text,
        Err(error) => return fail(&format!("cannot read `{}`: {error}", list.display())),
    };
    let parsed_list = match NextestList::parse(&list_text) {
        Ok(parsed) => parsed,
        Err(error) => return fail(&format!("cannot parse `{}`: {error}", list.display())),
    };
    let junit_text = match std::fs::read_to_string(junit) {
        Ok(text) => text,
        Err(error) => return fail(&format!("cannot read `{}`: {error}", junit.display())),
    };
    let doctests = match doctest {
        None => None,
        Some(path) => match std::fs::read_to_string(path) {
            Ok(text) => Some(DoctestSummary::parse(&text)),
            Err(error) => return fail(&format!("cannot read `{}`: {error}", path.display())),
        },
    };
    let config_text = std::fs::read_to_string(root.join(nextest_config)).unwrap_or_default();
    let rerun = rerun_of.as_ref().map(|receipt_id| Rerun {
        receipt_id: receipt_id.clone(),
        first_failure: first_failure.as_ref().and_then(|path| {
            let text = std::fs::read_to_string(path).ok()?;
            serde_json::from_str::<Vec<String>>(&text).ok()
        }),
    });

    let selected: Vec<PackageRow> = if packages.is_empty() {
        collected.packages.clone()
    } else {
        collected
            .packages
            .iter()
            .filter(|package| packages.contains(&package.name))
            .cloned()
            .collect()
    };
    let mut declared_targets = collected.declared_targets(!default_features_only);
    if !packages.is_empty() {
        declared_targets.retain(|name, _| packages.contains(name));
    }

    let input = FlakeInput {
        list: parsed_list,
        junit: JUnitReport::parse(&junit_text),
        profile: profile.clone(),
        profile_retries: profile_retries(&config_text),
        env_retries: std::env::var("NEXTEST_RETRIES").ok(),
        filter: filter.clone(),
        declared_targets,
        doctest_crates: aex_workspace_check::collect::doctest_crates(&selected, &metadata),
        doctests,
        source: collected.scan.clone(),
        rerun,
    };
    let report = aex_workspace_check::flake::scan(&input);
    match serde_json::to_string_pretty(&report.inventory) {
        Ok(text) => println!("{text}"),
        Err(error) => return fail(&error.to_string()),
    }
    if report.violations.is_empty() {
        return ExitCode::SUCCESS;
    }
    eprintln!(
        "aex-workspace-check: {} violation(s)",
        report.violations.len()
    );
    for violation in &report.violations {
        eprintln!("  {violation}");
    }
    ExitCode::FAILURE
}

fn fail(message: &str) -> ExitCode {
    eprintln!("aex-workspace-check: {message}");
    ExitCode::FAILURE
}
