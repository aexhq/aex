//! Source and plan policies: "Terraform packages no code", "no compiler in a
//! release job", and the workflow structural gates.
//!
//! These are the checks that keep the build boundary from eroding one
//! convenience at a time. Each is a scanner over checked-in text, so a
//! violation is caught in review rather than discovered when a deploy job
//! quietly compiles something.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Exit, Result, ToolError, Violation, io};

// ---------------------------------------------------------------------------
// Terraform source policy
// ---------------------------------------------------------------------------

/// Constructs that would let Terraform build, fetch or bundle code.
const FORBIDDEN_TERRAFORM: &[(&str, &str)] = &[
    ("data \"archive_file\"", "terraform-packages-code"),
    ("data \"external\"", "terraform-packages-code"),
    ("data \"http\"", "terraform-packages-code"),
    ("resource \"null_resource\"", "terraform-packages-code"),
    ("provisioner \"", "terraform-packages-code"),
    ("local-exec", "terraform-packages-code"),
    ("remote-exec", "terraform-packages-code"),
    ("aws_lambda_invocation", "terraform-invokes-code"),
    ("resource \"docker_", "terraform-packages-code"),
    ("filebase64sha256(", "terraform-hashes-local-build"),
];

/// Functions a module may not call, because a module that reads a file is a
/// module `mock_provider` cannot test and a self-hoster cannot vendor.
const FORBIDDEN_MODULE_FUNCTIONS: &[&str] = &["file(", "fileset(", "filemd5(", "templatefile("];

/// Scan the Terraform tree.
///
/// # Errors
/// Returns [`Exit::TerraformPolicy`] carrying every violation.
pub fn scan_terraform(root: &Path) -> Result<usize> {
    let mut violations = Vec::new();
    let mut scanned = 0usize;
    let infra = root.join("infra");
    if !infra.is_dir() {
        return Ok(0);
    }
    for entry in walkdir::WalkDir::new(&infra)
        .into_iter()
        .filter_entry(|entry| entry.file_name() != ".terraform")
    {
        let entry = entry.map_err(|err| {
            ToolError::single(Exit::TerraformPolicy, "io", format!("walking infra: {err}"))
        })?;
        if entry
            .path()
            .extension()
            .is_none_or(|ext| !ext.eq_ignore_ascii_case("tf"))
        {
            continue;
        }
        scanned += 1;
        let relative = entry
            .path()
            .strip_prefix(root)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .replace('\\', "/");
        let text = std::fs::read_to_string(entry.path()).map_err(|err| io(&relative, &err))?;
        violations.extend(scan_terraform_text(&relative, &text));
    }
    if violations.is_empty() {
        Ok(scanned)
    } else {
        Err(ToolError::many(Exit::TerraformPolicy, violations))
    }
}

/// Scan one Terraform document.
#[must_use]
pub fn scan_terraform_text(path: &str, text: &str) -> Vec<Violation> {
    let mut violations = Vec::new();
    let is_module = path.contains("infra/modules/");
    for (number, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }
        for (needle, rule) in FORBIDDEN_TERRAFORM {
            if trimmed.contains(needle) {
                violations.push(Violation::new(
                    *rule,
                    format!(
                        "{path}:{}: `{}` would make Terraform build, fetch or invoke code",
                        number + 1,
                        needle.trim_end_matches(" \"")
                    ),
                ));
            }
        }
        if trimmed.contains("aws_lambda_function")
            && text.contains("filename")
            && text.contains("aws_lambda_function")
        {
            // Reported once, below, rather than per line.
        }
        if is_module {
            for function in FORBIDDEN_MODULE_FUNCTIONS {
                if trimmed.contains(function) {
                    violations.push(Violation::new(
                        "terraform-module-reads-file",
                        format!(
                            "{path}:{}: a module may not call `{}`; only a root reads the \
                             two checked-in generated bundles",
                            number + 1,
                            function.trim_end_matches('(')
                        ),
                    ));
                }
            }
        }
    }
    if let Some(block) = find_block(text, "resource \"aws_lambda_function\"")
        && block.lines().any(|line| {
            let trimmed = line.trim();
            trimmed.starts_with("filename") && trimmed.contains('=')
        })
    {
        violations.push(Violation::new(
            "terraform-packages-code",
            format!(
                "{path}: an `aws_lambda_function` declares `filename`; code comes from an \
                 immutable object the release lane already published"
            ),
        ));
    }
    violations
}

fn find_block<'a>(text: &'a str, header: &str) -> Option<&'a str> {
    let start = text.find(header)?;
    let rest = &text[start..];
    let mut depth = 0i32;
    for (offset, ch) in rest.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&rest[..=offset]);
                }
            }
            _ => {}
        }
    }
    Some(rest)
}

// ---------------------------------------------------------------------------
// Dockerfile policy
// ---------------------------------------------------------------------------

/// Instructions a product image may use. Anything else means a compiler,
/// package manager or network fetch runs inside the image build, and the image
/// stops being reproducible from the recorded inputs.
const ALLOWED_DOCKERFILE_INSTRUCTIONS: &[&str] = &[
    "FROM",
    "COPY",
    "USER",
    "EXPOSE",
    "ENTRYPOINT",
    "CMD",
    "ENV",
    "WORKDIR",
    "LABEL",
    "STOPSIGNAL",
    "ARG",
    "HEALTHCHECK",
];

/// Scan one Dockerfile for the COPY-only policy.
#[must_use]
pub fn scan_dockerfile(path: &str, text: &str) -> Vec<Violation> {
    let mut violations = Vec::new();
    let mut stages = 0usize;
    for (number, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let instruction = trimmed
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        if instruction == "FROM" {
            stages += 1;
            if !trimmed.contains("@sha256:") {
                violations.push(Violation::new(
                    "dockerfile-base-not-pinned",
                    format!(
                        "{path}:{}: the base image is not pinned by digest",
                        number + 1
                    ),
                ));
            }
        }
        if !ALLOWED_DOCKERFILE_INSTRUCTIONS.contains(&instruction.as_str()) {
            violations.push(Violation::new(
                "dockerfile-not-copy-only",
                format!(
                    "{path}:{}: `{instruction}` is not permitted; product images are \
                     COPY-only over a digest-pinned base",
                    number + 1
                ),
            ));
        }
        if trimmed.contains("--mount=type=cache") {
            violations.push(Violation::new(
                "dockerfile-not-copy-only",
                format!(
                    "{path}:{}: a build cache mount makes the image a function of the \
                     builder's state",
                    number + 1
                ),
            ));
        }
    }
    if stages > 1 {
        violations.push(Violation::new(
            "dockerfile-builder-stage",
            format!(
                "{path}: {stages} build stages; a builder stage runs a compiler inside the \
                 image build"
            ),
        ));
    }
    violations
}

// ---------------------------------------------------------------------------
// Workflow structural gates
// ---------------------------------------------------------------------------

/// Runner labels that do not exist. A leftover label does not fail the run: the
/// job queues for a runner that will never appear, is silently discarded after
/// a day, and reports nothing at all.
const BANNED_RUNNER_LABELS: &[&str] = &["self-hosted", "ec2-spot", "aex-home", "ghr-"];

/// Indirection variables from the retired runner fleets.
const BANNED_RUNNER_VARIABLES: &[&str] =
    &["AEX_CI_RUNNER", "AEX_CI_RUNNER_LARGE", "AEX_CI_RUNNER_ARM"];

/// Commands that build, bundle, generate or package. None may appear in a job
/// that holds a deploy credential.
const BUILD_COMMANDS: &[&str] = &[
    "cargo build",
    "cargo lambda build",
    "cargo zigbuild",
    "cargo install",
    "bun build",
    "bun run build",
    "npm run build",
    "docker build",
    "docker buildx",
    "tsc ",
    "aex-contract-gen build",
    "artifact package",
];

/// Permission scopes that publish.
const PUBLISH_SCOPES: &[&str] = &["packages", "attestations"];

/// The result of linting the workflow tree.
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowReport {
    /// Schema discriminator.
    pub schema: &'static str,
    /// Files linted.
    pub files: Vec<String>,
    /// How many `uses:` pins were checked.
    pub pins: usize,
}

/// Lint every workflow under `.github/workflows`.
///
/// # Errors
/// Returns [`Exit::TerraformPolicy`] — the shared source-policy code — carrying
/// every violation. Workflow policy and Terraform policy are the same class of
/// failure from a caller's point of view: a checked-in file declares something
/// the build boundary forbids.
pub fn lint_workflows(root: &Path) -> Result<WorkflowReport> {
    let dir = root.join(".github/workflows");
    let mut files = Vec::new();
    let mut violations = Vec::new();
    let mut pins = 0usize;
    let entries = std::fs::read_dir(&dir).map_err(|err| io(&dir.display().to_string(), &err))?;
    let mut paths: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext == "yml" || ext == "yaml")
        })
        .collect();
    paths.sort();
    for path in paths {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        let text =
            std::fs::read_to_string(&path).map_err(|err| io(&path.display().to_string(), &err))?;
        let (file_violations, file_pins) = lint_workflow_text(&name, &text);
        violations.extend(file_violations);
        pins += file_pins;
        files.push(name);
    }
    if files.is_empty() {
        violations.push(Violation::new(
            "workflow-tree-empty",
            ".github/workflows holds no workflow; a repository with no lane has no gate",
        ));
    }
    if violations.is_empty() {
        Ok(WorkflowReport {
            schema: "aex.workflow-report.v1",
            files,
            pins,
        })
    } else {
        Err(ToolError::many(Exit::TerraformPolicy, violations))
    }
}

/// Lint one workflow document, returning its violations and pin count.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn lint_workflow_text(name: &str, text: &str) -> (Vec<Violation>, usize) {
    let mut violations = Vec::new();

    // Line scan: every `uses:` is a 40-hex SHA carrying a trailing tag comment.
    let mut line_uses = 0usize;
    for (number, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        for label in BANNED_RUNNER_LABELS {
            if trimmed.contains(label) {
                violations.push(Violation::new(
                    "workflow-banned-runner",
                    format!(
                        "{name}:{}: `{label}` names a runner fleet that does not exist; the \
                         job would queue for a day and then be discarded silently",
                        number + 1
                    ),
                ));
            }
        }
        for variable in BANNED_RUNNER_VARIABLES {
            if trimmed.contains(variable) {
                violations.push(Violation::new(
                    "workflow-banned-runner",
                    format!(
                        "{name}:{}: `{variable}` reintroduces runner-selector indirection",
                        number + 1
                    ),
                ));
            }
        }
        if trimmed.contains("NODE_AUTH_TOKEN") {
            violations.push(Violation::new(
                "workflow-npm-token",
                format!(
                    "{name}:{}: publication uses OIDC provenance, not a long-lived npm token",
                    number + 1
                ),
            ));
        }
        let Some(rest) = trimmed
            .strip_prefix("- uses:")
            .or_else(|| trimmed.strip_prefix("uses:"))
        else {
            continue;
        };
        line_uses += 1;
        let reference = rest.trim();
        // A reusable workflow in this repository is referenced by path.
        if reference.starts_with("./") {
            continue;
        }
        let (action, comment) = reference.split_once('#').unwrap_or((reference, ""));
        let action = action.trim();
        let Some((_, pin)) = action.rsplit_once('@') else {
            violations.push(Violation::new(
                "workflow-action-unpinned",
                format!("{name}:{}: `{action}` names no version at all", number + 1),
            ));
            continue;
        };
        if pin.len() != 40 || !pin.chars().all(|ch| ch.is_ascii_hexdigit()) {
            violations.push(Violation::new(
                "workflow-action-unpinned",
                format!(
                    "{name}:{}: `{action}` is pinned to `{pin}`; a moving tag is a moving \
                     dependency",
                    number + 1
                ),
            ));
        }
        if comment.trim().is_empty() {
            violations.push(Violation::new(
                "workflow-action-pin-uncommented",
                format!(
                    "{name}:{}: `{action}` carries no trailing `# vX.Y.Z` comment, so the \
                     pin cannot be reviewed or bumped",
                    number + 1
                ),
            ));
        }
    }

    // Structural scan over the parsed document.
    let Ok(document) = serde_norway::from_str::<serde_json::Value>(text) else {
        violations.push(Violation::new(
            "workflow-unparseable",
            format!("{name} does not parse as YAML"),
        ));
        return (violations, line_uses);
    };

    let parsed_uses = count_uses(&document);
    if parsed_uses != line_uses {
        violations.push(Violation::new(
            "workflow-pin-scan-disagreement",
            format!(
                "{name}: the line scan found {line_uses} `uses:` and the parser found \
                 {parsed_uses}; the scanner cannot be allowed to under-report"
            ),
        ));
    }

    // YAML 1.1 readers resolve a bare `on:` key to the boolean `true`. Look
    // under both spellings so quoting the key is a style choice rather than a
    // way to slip past the gate.
    let triggers = document
        .get("on")
        .or_else(|| document.get("true"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    if triggers.get("pull_request_target").is_some() {
        violations.push(Violation::new(
            "workflow-pull-request-target",
            format!(
                "{name}: `pull_request_target` runs with repository secrets against a fork's \
                 proposal"
            ),
        ));
    }

    if let Some(concurrency) = document.get("concurrency").and_then(|c| c.as_object()) {
        for key in concurrency.keys() {
            if !matches!(key.as_str(), "group" | "cancel-in-progress") {
                violations.push(Violation::new(
                    "workflow-concurrency-key",
                    format!("{name}: concurrency declares unknown key `{key}`"),
                ));
            }
        }
    }

    let is_release_engine = name.starts_with("_release-engine");
    if let Some(jobs) = document.get("jobs").and_then(|jobs| jobs.as_object()) {
        for (job_name, job) in jobs {
            let reusable = job.get("uses").is_some();
            if let Some(runs_on) = job.get("runs-on") {
                if runs_on.as_str() != Some("ubuntu-latest") {
                    violations.push(Violation::new(
                        "workflow-runner-not-github-hosted",
                        format!(
                            "{name}: job `{job_name}` declares runs-on `{runs_on}`; every job \
                             runs on GitHub-hosted `ubuntu-latest`"
                        ),
                    ));
                }
            } else if !reusable {
                violations.push(Violation::new(
                    "workflow-runner-undeclared",
                    format!("{name}: job `{job_name}` declares no runs-on"),
                ));
            }

            let permissions = job.get("permissions");
            if permissions.is_none() && !reusable && document.get("permissions").is_none() {
                violations.push(Violation::new(
                    "workflow-permissions-undeclared",
                    format!(
                        "{name}: job `{job_name}` declares no permissions and the workflow \
                         declares no default"
                    ),
                ));
            }
            if let Some(scopes) = permissions.and_then(|p| p.as_object()) {
                let held: BTreeSet<&str> = scopes
                    .iter()
                    .filter(|(_, value)| value.as_str() != Some("none"))
                    .map(|(key, _)| key.as_str())
                    .collect();
                let publishes = held.iter().any(|scope| PUBLISH_SCOPES.contains(scope));
                let deploys = held.contains("id-token") && job_deploys(job);
                if publishes && deploys {
                    violations.push(Violation::new(
                        "workflow-permission-overlap",
                        format!(
                            "{name}: job `{job_name}` holds both publish and deploy scopes; a \
                             build job may not assume a deploy role"
                        ),
                    ));
                }
            }

            if is_release_engine {
                for command in job_run_commands(job) {
                    for build in BUILD_COMMANDS {
                        if command.contains(build) {
                            violations.push(Violation::new(
                                "release-engine-builds",
                                format!(
                                    "{name}: job `{job_name}` runs `{build}`; the release \
                                     engine deploys bytes somebody else already built"
                                ),
                            ));
                        }
                    }
                }
            }
        }
    }
    (violations, line_uses)
}

/// Whether a job mutates a deployed plane.
///
/// Uploading bytes to an immutable object store is publication, not
/// deployment; conflating the two would forbid the one job that must do both
/// build and upload. What marks a deploy is applying a saved plan or holding a
/// GitHub Environment.
fn job_deploys(job: &serde_json::Value) -> bool {
    job_run_commands(job)
        .iter()
        .any(|command| command.contains("terraform apply") || command.contains("terraform destroy"))
        || job.get("environment").is_some()
}

fn job_run_commands(job: &serde_json::Value) -> Vec<String> {
    job.get("steps")
        .and_then(|steps| steps.as_array())
        .map(|steps| {
            steps
                .iter()
                .filter_map(|step| step.get("run").and_then(|run| run.as_str()))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn count_uses(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Object(members) => members
            .iter()
            .map(|(key, child)| usize::from(key == "uses" && child.is_string()) + count_uses(child))
            .sum(),
        serde_json::Value::Array(items) => items.iter().map(count_uses).sum(),
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Terraform plan policy
// ---------------------------------------------------------------------------

/// `release/policy/terraform-policy.toml`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlanPolicy {
    /// Schema discriminator.
    pub schema: String,
    /// Resource types that may never be created by any plan.
    #[serde(default)]
    pub forbidden_resource_types: Vec<String>,
    /// Resource types whose deletion always needs an explicit decision.
    #[serde(default)]
    pub protected_resource_types: Vec<String>,
    /// Tags every managed resource must carry.
    #[serde(default)]
    pub required_tags: Vec<String>,
}

/// What one plan does.
#[derive(Debug, Clone, Serialize)]
pub struct PlanSummary {
    /// Schema discriminator.
    pub schema: &'static str,
    /// Creates.
    pub create: Vec<String>,
    /// Updates.
    pub update: Vec<String>,
    /// Replacements.
    pub replace: Vec<String>,
    /// Deletes.
    pub delete: Vec<String>,
    /// Which attributes changed, by resource address.
    pub changed_attributes: BTreeMap<String, Vec<String>>,
}

/// Summarize a `terraform show -json` plan document.
///
/// # Errors
/// Returns [`Exit::TerraformPolicy`] when the document is not a plan.
pub fn summarize_plan(plan: &serde_json::Value) -> Result<PlanSummary> {
    let changes = plan
        .get("resource_changes")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            ToolError::single(
                Exit::TerraformPolicy,
                "plan-unparseable",
                "the document has no `resource_changes` array",
            )
        })?;
    let mut summary = PlanSummary {
        schema: "aex.plan-summary.v1",
        create: Vec::new(),
        update: Vec::new(),
        replace: Vec::new(),
        delete: Vec::new(),
        changed_attributes: BTreeMap::new(),
    };
    for change in changes {
        let address = change
            .get("address")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let actions: Vec<&str> = change
            .get("change")
            .and_then(|c| c.get("actions"))
            .and_then(serde_json::Value::as_array)
            .map(|actions| {
                actions
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect()
            })
            .unwrap_or_default();
        match actions.as_slice() {
            ["create"] => summary.create.push(address.clone()),
            ["update"] => summary.update.push(address.clone()),
            ["delete"] => summary.delete.push(address.clone()),
            ["delete", "create"] | ["create", "delete"] => {
                summary.replace.push(address.clone());
            }
            _ => {}
        }
        if let (Some(before), Some(after)) = (
            change.get("change").and_then(|c| c.get("before")),
            change.get("change").and_then(|c| c.get("after")),
        ) && let (Some(before), Some(after)) = (before.as_object(), after.as_object())
        {
            let mut moved: Vec<String> = Vec::new();
            for (key, value) in after {
                if before.get(key) != Some(value) {
                    moved.push(key.clone());
                }
            }
            moved.sort();
            if !moved.is_empty() {
                summary.changed_attributes.insert(address, moved);
            }
        }
    }
    Ok(summary)
}

/// Apply the plan policy.
///
/// # Errors
/// Returns [`Exit::TerraformPolicy`] carrying every violation.
pub fn check_plan(summary: &PlanSummary, policy: &PlanPolicy) -> Result<()> {
    let mut violations = Vec::new();
    for address in summary.create.iter().chain(&summary.replace) {
        for forbidden in &policy.forbidden_resource_types {
            if address.contains(forbidden) {
                violations.push(Violation::new(
                    "plan-forbidden-resource",
                    format!(
                        "`{address}` would create `{forbidden}`, which packages or invokes code"
                    ),
                ));
            }
        }
    }
    for address in summary.delete.iter().chain(&summary.replace) {
        for protected in &policy.protected_resource_types {
            if address.contains(protected) {
                violations.push(Violation::new(
                    "plan-destroys-protected-resource",
                    format!(
                        "`{address}` would destroy or replace a `{protected}`; durable state \
                         is not replaced by a routine plan"
                    ),
                ));
            }
        }
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(ToolError::many(Exit::TerraformPolicy, violations))
    }
}

/// Attributes an artifact change is permitted to move.
const ARTIFACT_ATTRIBUTES: &[&str] = &[
    "s3_key",
    "s3_object_version",
    "source_code_hash",
    "image_uri",
    "image",
    "container_definitions",
    "task_definition",
    "qualified_arn",
    "version",
];

/// Assert a manifest-only change moves exactly the artifact attributes, and a
/// Terraform-only change moves none of them.
///
/// # Errors
/// Returns [`Exit::TerraformPolicy`] when the plan mixes the two.
pub fn check_change_isolation(summary: &PlanSummary, manifest_only: bool) -> Result<()> {
    let mut violations = Vec::new();
    for (address, attributes) in &summary.changed_attributes {
        let artifact: Vec<&String> = attributes
            .iter()
            .filter(|attribute| ARTIFACT_ATTRIBUTES.contains(&attribute.as_str()))
            .collect();
        let other: Vec<&String> = attributes
            .iter()
            .filter(|attribute| !ARTIFACT_ATTRIBUTES.contains(&attribute.as_str()))
            .collect();
        if manifest_only && !other.is_empty() {
            violations.push(Violation::new(
                "plan-change-not-isolated",
                format!(
                    "`{address}` moves non-artifact attribute(s) {other:?} for a manifest-only \
                     change"
                ),
            ));
        }
        if !manifest_only && !artifact.is_empty() {
            violations.push(Violation::new(
                "plan-change-not-isolated",
                format!(
                    "`{address}` moves artifact attribute(s) {artifact:?} for an infrastructure-only \
                     change"
                ),
            ));
        }
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(ToolError::many(Exit::TerraformPolicy, violations))
    }
}
