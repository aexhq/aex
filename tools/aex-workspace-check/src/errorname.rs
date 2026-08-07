//! An error enum name occurs in exactly one file.
//!
//! `ConfigError` was declared in 28 files and `RunError` in 22, and the two
//! `RunError`s did not even mean the same thing: in `aex-session-domain` it is
//! "why a run transition was refused", a product concept, and in fifteen
//! binaries it is "the process failed to start". Grepping an error string
//! returned both with no way to rank them, which is the first search an agent
//! runs when handed a stack trace.
//!
//! # Invariants
//!
//! - uniqueness is judged per **file**, not per crate: `aex-regional-http`
//!   declares `ProjectionError` twice within itself, and a crate-scoped check
//!   misses it entirely
//! - `enum` counts as well as `pub enum`: visibility does not make a name less
//!   ambiguous to `grep`, and a check that only sees `pub` can be satisfied
//!   while the invariant is violated
//! - the grandfathered table freezes name → declaring files, never a bare name,
//!   so a further file cannot join a known-colliding name without failing

use std::collections::BTreeMap;
use std::path::Path;

use crate::metadata::WorkspaceMetadata;
use crate::rules::Violation;

/// Names that still collide, each with the exact files that declare it.
///
/// Shrink-only. There is no writer and no number here: the only legal edit is
/// deleting a row or a file from a row, so two branches that each add a
/// declaration both fail on their own branch and cannot merge-sum their way
/// back to green the way the retired module-size baseline could.
pub const GRANDFATHERED: &[(&str, &[&str])] = &[
    ("AdmissionError", &["crates/aex-observation-app/src/use_cases.rs", "crates/aex-usage-domain/src/fact.rs", "services/regional-secret-api/src/lib.rs", "services/regional-session-api/src/admission.rs"]),
    ("AuthorityError", &["services/finance-api/src/authority.rs", "services/regional-otlp/src/authority.rs"]),
    ("BudgetError", &["crates/aex-brain-domain/src/budget.rs", "tests/support/aex-test-harness/src/budget.rs"]),
    ("CanonicalError", &["crates/aex-usage-domain/src/intent.rs", "crates/aex-wire/src/canonical.rs"]),
    ("CatalogError", &["crates/aex-brain-app/src/ports/catalog.rs", "crates/aex-model-catalog/src/qualified.rs", "crates/aex-runtime-control/src/catalog.rs"]),
    ("ClaimError", &["crates/aex-brain-app/src/ports/store.rs", "workers/session-operation-worker/src/lib.rs"]),
    ("CommitError", &["crates/aex-brain-app/src/ports/store.rs", "crates/aex-session-app/src/ports.rs", "workers/session-operation-worker/src/lib.rs"]),
    ("CompositionError", &["crates/aex-central-http/src/capability.rs", "crates/aex-regional-http/src/capability.rs"]),
    ("CursorError", &["crates/aex-control-domain/src/cursor.rs", "crates/aex-observation-query/src/cursor.rs", "crates/aex-operation-domain/src/cursor.rs", "crates/aex-regional-http/src/cursor.rs", "crates/aex-session-dynamodb/src/paging.rs"]),
    ("DecodeError", &["crates/aex-rds-data/src/error.rs", "crates/aex-usage-compute-dynamodb/src/codec.rs", "crates/aex-usage-storage-dynamodb/src/codec.rs", "crates/aex-usage-transfer-dynamodb/src/codec.rs"]),
    ("DispatchError", &["workers/usage-compute-worker/src/main.rs", "workers/usage-receipt-dispatcher/src/outbox.rs", "workers/usage-storage-worker/src/main.rs", "workers/usage-transfer-worker/src/main.rs"]),
    ("DownloadError", &["services/finance-api/src/download.rs", "tools/aex-cli/src/download.rs"]),
    ("DrainError", &["crates/aex-usage-app/src/probe/sink.rs", "runtimes/brain-mux/src/drain.rs"]),
    ("EdgeError", &["crates/aex-central-http/src/error.rs", "crates/aex-regional-http/src/error.rs"]),
    ("EffectError", &["crates/aex-brain-domain/src/effect.rs", "crates/aex-control-app/src/ports.rs"]),
    ("EncodeError", &["crates/aex-content-dynamodb/src/codec.rs", "crates/aex-observation-export/src/encoder.rs", "crates/aex-registry-dynamodb/src/codec.rs", "crates/aex-runtime-activity-dynamodb/src/codec.rs", "crates/aex-secret-custody-dynamodb/src/codec.rs", "crates/aex-work-dynamodb/src/codec.rs"]),
    ("EnvelopeError", &["crates/aex-regional-http/src/envelope.rs", "crates/aex-secret-aws/src/envelope.rs", "crates/aex-usage-domain/src/ingress.rs"]),
    ("FoldError", &["crates/aex-brain-domain/src/fold.rs", "crates/aex-usage-app/src/projection.rs"]),
    ("FrameError", &["crates/aex-hands-agent/src/wire.rs", "services/regional-observation-api/src/ndjson.rs"]),
    ("FrontierError", &["crates/aex-observation-domain/src/frontier.rs", "crates/aex-usage-domain/src/frontier.rs"]),
    ("HandsError", &["crates/aex-brain-app/src/ports/hands.rs", "crates/aex-brain-hands/src/adapter.rs"]),
    ("IdentityError", &["crates/aex-identity-app/src/use_cases.rs", "crates/aex-regional-http/src/idempotency.rs", "crates/aex-usage-domain/src/identity.rs"]),
    ("ImageError", &["runtimes/hands-agent/src/image.rs", "tests/support/aex-test-harness/src/images.rs"]),
    ("JournalError", &["crates/aex-hands-agent/src/journal.rs", "crates/aex-session-domain/src/journal.rs"]),
    ("KeyError", &["crates/aex-observation-domain/src/keys.rs", "crates/aex-session-dynamodb/src/component.rs", "crates/aex-usage-domain/src/keys.rs"]),
    ("MoneyError", &["crates/aex-finance-domain/src/money.rs", "crates/aex-internal-contracts/src/money.rs"]),
    ("PageError", &["crates/aex-operation-domain/src/cursor.rs", "crates/aex-regional-http/src/page.rs"]),
    ("PlanError", &["crates/aex-brain-store-dynamodb/src/plan.rs", "crates/aex-session-app/src/plan.rs"]),
    ("PoolError", &["crates/aex-brain-mcp/src/pool.rs", "crates/aex-brain-provider-gateway/src/pool.rs"]),
    ("PortError", &["crates/aex-observation-app/src/ports.rs", "crates/aex-session-app/src/ports.rs", "crates/aex-usage-app/src/ports.rs"]),
    ("PrefixError", &["crates/aex-brain-test-support/src/prefix.rs", "crates/aex-central-test-support/src/prefix.rs", "crates/aex-observation-test-support/src/prefix.rs", "crates/aex-regional-test-support/src/prefix.rs"]),
    ("ProjectionError", &["crates/aex-regional-http/src/edge.rs", "crates/aex-regional-http/src/projection.rs"]),
    ("QualificationError", &["crates/aex-brain-mcp/src/client.rs", "tests/live/aex-live-model-catalog/src/deepseek_qualification.rs"]),
    ("QueryError", &["crates/aex-observation-query/src/ast.rs", "crates/aex-usage-query-dynamodb/src/expressions.rs"]),
    ("ReadinessError", &["crates/aex-regional-http/src/health.rs", "tests/live/aex-live-model-catalog/src/executor.rs"]),
    ("ReceiptError", &["crates/aex-hands-protocol/src/lifecycle.rs", "crates/aex-usage-compute-dynamodb/src/stream.rs", "crates/aex-usage-storage-dynamodb/src/stream.rs", "crates/aex-usage-transfer-dynamodb/src/stream.rs", "tests/live/aex-live-model-catalog/src/lib.rs"]),
    ("RowError", &["crates/aex-usage-compute-dynamodb/src/attribute.rs", "crates/aex-usage-storage-dynamodb/src/attribute.rs", "crates/aex-usage-transfer-dynamodb/src/attribute.rs"]),
    ("ScopeError", &["crates/aex-control-domain/src/scope.rs", "crates/aex-session-dynamodb/src/replay.rs"]),
    ("SinkError", &["crates/aex-runtime-control/src/usage.rs", "crates/aex-usage-app/src/probe/sink.rs"]),
    ("StoreError", &["crates/aex-brain-app/src/ports/store.rs", "crates/aex-identity-app/src/ports.rs", "crates/aex-observation-store-dynamodb/src/store.rs", "crates/aex-session-dynamodb/src/error.rs", "crates/aex-usage-compute-dynamodb/src/expressions.rs", "crates/aex-usage-storage-dynamodb/src/expressions.rs", "crates/aex-usage-transfer-dynamodb/src/expressions.rs"]),
    ("StreamError", &["crates/aex-usage-compute-dynamodb/src/stream.rs", "crates/aex-usage-storage-dynamodb/src/stream.rs", "crates/aex-usage-transfer-dynamodb/src/stream.rs"]),
    ("TransitionError", &["crates/aex-observation-domain/src/batch.rs", "crates/aex-operation-domain/src/operation.rs"]),
    ("ValueError", &["crates/aex-wire/src/types.rs", "crates/aex-workspace-domain/src/registry.rs"]),
];

/// Every `*Error` enum a source file declares.
///
/// Matches `enum <Name>Error` and `pub enum <Name>Error` at the start of a
/// line, which is how every declaration in this workspace is written.
#[must_use]
pub fn declarations(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        let rest = trimmed
            .strip_prefix("pub enum ")
            .or_else(|| trimmed.strip_prefix("enum "));
        let Some(rest) = rest else { continue };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if name.ends_with("Error") && name.len() > "Error".len() {
            names.push(name);
        }
    }
    names
}

/// Scans every member's sources for error-enum declarations.
///
/// # Errors
///
/// Returns [`crate::collect::CollectError`] when a source file cannot be read.
pub fn scan(
    root: &Path,
    metadata: &WorkspaceMetadata,
) -> Result<BTreeMap<String, Vec<String>>, crate::collect::CollectError> {
    let mut found: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let directories = metadata.member_directories().unwrap_or_default();
    for directory in directories.values() {
        let base = directory
            .split('/')
            .fold(root.to_path_buf(), |accumulator, segment| {
                accumulator.join(segment)
            });
        for sub in ["src", "tests", "benches"] {
            walk(&base.join(sub), root, &mut found)?;
        }
    }
    Ok(found)
}

fn walk(
    directory: &Path,
    root: &Path,
    found: &mut BTreeMap<String, Vec<String>>,
) -> Result<(), crate::collect::CollectError> {
    if !directory.is_dir() {
        return Ok(());
    }
    let entries = std::fs::read_dir(directory).map_err(|source| crate::collect::CollectError::Read {
        path: directory.display().to_string(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| crate::collect::CollectError::Read {
            path: directory.display().to_string(),
            source,
        })?;
        let path = entry.path();
        if path.is_dir() {
            walk(&path, root, found)?;
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).map_err(|source| {
            crate::collect::CollectError::Read {
                path: path.display().to_string(),
                source,
            }
        })?;
        let relative = path
            .strip_prefix(root)
            .map_or_else(|_| path.clone(), Path::to_path_buf);
        let relative = crate::metadata::normalise(&relative);
        for name in declarations(&source) {
            found.entry(name).or_default().push(relative.clone());
        }
    }
    Ok(())
}

/// No error enum name is declared in two files, except where grandfathered.
#[must_use]
pub fn check(found: &BTreeMap<String, Vec<String>>) -> Vec<Violation> {
    check_against(found, GRANDFATHERED)
}

/// [`check`] against an explicit grandfathered table.
///
/// Separate so a unit test can exercise the rule without the real 43-row table
/// reporting every one of its names as "declared nowhere" against a two-entry
/// fixture.
#[must_use]
pub fn check_against(
    found: &BTreeMap<String, Vec<String>>,
    grandfathered: &[(&str, &[&str])],
) -> Vec<Violation> {
    const RULE: &str = "error-name-unique";
    let frozen: BTreeMap<&str, &[&str]> = grandfathered.iter().copied().collect();
    let mut violations = Vec::new();

    for (name, files) in found {
        let mut files = files.clone();
        files.sort();
        files.dedup();
        match frozen.get(name.as_str()) {
            None if files.len() > 1 => violations.push(Violation {
                rule: RULE,
                detail: format!(
                    "`{name}` is declared in {} files ({}); prefix each with its owning package",
                    files.len(),
                    files.join(", ")
                ),
            }),
            Some(allowed) => {
                for file in &files {
                    if !allowed.contains(&file.as_str()) {
                        violations.push(Violation {
                            rule: RULE,
                            detail: format!(
                                "`{name}` gained a declaration in `{file}`; the grandfathered list shrinks, it does not grow"
                            ),
                        });
                    }
                }
                for allowed_file in *allowed {
                    if !files.iter().any(|file| file == allowed_file) {
                        violations.push(Violation {
                            rule: RULE,
                            detail: format!(
                                "`{name}` no longer declared in `{allowed_file}`; remove that row from the grandfathered list"
                            ),
                        });
                    }
                }
            }
            None => {}
        }
    }
    for (name, _) in grandfathered {
        if !found.contains_key(*name) {
            violations.push(Violation {
                rule: RULE,
                detail: format!(
                    "`{name}` is grandfathered but declared nowhere; remove it from the list"
                ),
            });
        }
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::{check_against, declarations};
    use std::collections::BTreeMap;

    #[test]
    fn both_visibilities_are_found() {
        let source = "pub enum ConfigError {}\nenum RunError {}\npub enum Outcome {}\n";
        assert_eq!(declarations(source), vec!["ConfigError", "RunError"]);
    }

    #[test]
    fn an_indented_declaration_is_found() {
        assert_eq!(declarations("    pub enum InnerError {}"), vec!["InnerError"]);
    }

    #[test]
    fn a_bare_error_is_not_a_declaration() {
        assert!(declarations("pub enum Error {}").is_empty());
    }

    #[test]
    fn one_file_per_name_is_silent() {
        let mut found = BTreeMap::new();
        found.insert("AError".to_owned(), vec!["crates/a/src/lib.rs".to_owned()]);
        assert!(check_against(&found, &[]).is_empty());
    }

    #[test]
    fn two_files_sharing_a_name_is_a_violation() {
        let mut found = BTreeMap::new();
        found.insert(
            "AError".to_owned(),
            vec!["crates/a/src/lib.rs".to_owned(), "crates/b/src/lib.rs".to_owned()],
        );
        let violations = check_against(&found, &[]);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].detail.contains("declared in 2 files"));
    }

    #[test]
    fn the_same_name_twice_in_one_file_is_not_a_collision() {
        let mut found = BTreeMap::new();
        found.insert(
            "AError".to_owned(),
            vec!["crates/a/src/lib.rs".to_owned(), "crates/a/src/lib.rs".to_owned()],
        );
        assert!(check_against(&found, &[]).is_empty());
    }
}
