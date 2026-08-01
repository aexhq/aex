//! Runs every workspace structural rule and reports the violations.
//!
//! Exit code 0 means the workspace matches the frozen inventory and every rule
//! holds. Any other exit code means a rule failed or the check itself could not
//! run; neither is ever reported as success.

use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = match Command::new(&cargo)
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output()
    {
        Ok(output) => output,
        Err(error) => {
            eprintln!("aex-workspace-check: cannot run `{cargo} metadata`: {error}");
            return ExitCode::FAILURE;
        }
    };
    if !output.status.success() {
        eprintln!("aex-workspace-check: `{cargo} metadata` failed");
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        return ExitCode::FAILURE;
    }
    let json = String::from_utf8_lossy(&output.stdout).into_owned();

    let violations = match aex_workspace_check::check_metadata_json(&json) {
        Ok(violations) => violations,
        Err(error) => {
            eprintln!("aex-workspace-check: {error}");
            return ExitCode::FAILURE;
        }
    };

    if violations.is_empty() {
        println!(
            "aex-workspace-check: {} member(s) satisfy every structural rule",
            aex_workspace_check::inventory::expected_members().len()
        );
        return ExitCode::SUCCESS;
    }

    eprintln!("aex-workspace-check: {} violation(s)", violations.len());
    for violation in &violations {
        eprintln!("  {violation}");
    }
    ExitCode::FAILURE
}
