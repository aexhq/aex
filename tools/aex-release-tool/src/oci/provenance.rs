//! Parse and cross-check the cryptographically verified `gh` attestation result.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{OciImageIdentity, OciWorkflowRun};
use crate::error::{Exit, Result, ToolError, Violation, io};

const VERIFICATION_MEDIA_TYPE: &str =
    "application/vnd.dev.sigstore.verificationresult+json;version=0.1";
const STATEMENT_TYPE: &str = "https://in-toto.io/Statement/v1";
const PREDICATE_TYPE: &str = "https://slsa.dev/provenance/v1";
const BUILD_TYPE: &str = "https://actions.github.io/buildtypes/workflow/v1";

/// Identity extracted from a successful official GitHub CLI verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OciVerifiedProvenance {
    /// SLSA predicate type.
    pub predicate_type: String,
    /// Exact digest-only subject repository.
    pub subject_name: String,
    /// Exact OCI subject digest.
    pub subject_digest: String,
    /// Reusable workflow identity from SLSA run details and the certificate.
    pub builder_id: String,
    /// Top-level workflow path recorded by the GitHub build type.
    pub caller_workflow_path: String,
    /// Source repository URL.
    pub source_repository_uri: String,
    /// Protected source ref.
    pub source_ref: String,
    /// Exact source commit.
    pub source_commit: String,
    /// Exact run and attempt URL.
    pub invocation_id: String,
    /// SHA-256 of the exact bundle file verified by `gh`.
    pub bundle_digest: String,
    /// Canonical SHA-256 of the verified `Sigstore` result.
    pub verification_result_digest: String,
}

struct Expected<'a> {
    image: &'a OciImageIdentity,
    workflow: &'a OciWorkflowRun,
    source_uri: String,
    invocation: String,
    dependency: String,
    bare_digest: &'a str,
}

impl<'a> Expected<'a> {
    fn new(image: &'a OciImageIdentity, workflow: &'a OciWorkflowRun) -> Self {
        let source_uri = format!("https://github.com/{}", image.source.repository);
        Self {
            invocation: format!(
                "{source_uri}/actions/runs/{}/attempts/{}",
                workflow.run_id, workflow.run_attempt
            ),
            dependency: format!("git+{source_uri}@{}", workflow.r#ref),
            bare_digest: image
                .output_digest
                .strip_prefix("sha256:")
                .unwrap_or_default(),
            image,
            workflow,
            source_uri,
        }
    }
}

struct Parsed<'a> {
    wrapper: &'a Value,
    result: &'a Value,
    statement: &'a Value,
    certificate: &'a Value,
    build_definition: &'a Value,
    workflow_parameter: &'a Value,
    github_parameters: &'a Value,
    run_details: &'a Value,
    subjects: &'a [Value],
    dependencies: &'a [Value],
    timestamps: &'a [Value],
}

impl<'a> Parsed<'a> {
    fn new(wrapper: &'a Value) -> Result<Self> {
        let result = at(wrapper, &["verificationResult"])?;
        let statement = at(result, &["statement"])?;
        let certificate = at(result, &["signature", "certificate"])?;
        let predicate = at(statement, &["predicate"])?;
        let build_definition = at(predicate, &["buildDefinition"])?;
        let workflow_parameter = at(build_definition, &["externalParameters", "workflow"])?;
        let github_parameters = at(build_definition, &["internalParameters", "github"])?;
        let run_details = at(predicate, &["runDetails"])?;
        Ok(Self {
            wrapper,
            result,
            statement,
            certificate,
            build_definition,
            workflow_parameter,
            github_parameters,
            run_details,
            subjects: array(
                statement,
                "subject",
                "verified statement has no subject array",
            )?,
            dependencies: array(
                build_definition,
                "resolvedDependencies",
                "verified statement has no resolved dependency array",
            )?,
            timestamps: array(
                result,
                "verifiedTimestamps",
                "verification result has no timestamp array",
            )?,
        })
    }
}

/// Parse the official `gh attestation verify --format json` output and require
/// it to agree with the image, protected workflow, source and exact attempt.
///
/// # Errors
/// Refuses invalid JSON, multiple results, a hostile statement/certificate, or
/// a bundle other than the exact bundle passed to `gh`.
pub fn verify(
    image: &OciImageIdentity,
    workflow: &OciWorkflowRun,
    verified_result: &Path,
    bundle: &Path,
) -> Result<OciVerifiedProvenance> {
    let verified_bytes = std::fs::read(verified_result)
        .map_err(|error| io(&verified_result.display().to_string(), &error))?;
    let bundle_bytes =
        std::fs::read(bundle).map_err(|error| io(&bundle.display().to_string(), &error))?;
    let values: Vec<Value> = serde_json::from_slice(&verified_bytes).map_err(|error| {
        ToolError::single(
            Exit::ProvenanceMissing,
            "oci-provenance-json",
            format!("verified provenance JSON is invalid: {error}"),
        )
    })?;
    let [wrapper] = values.as_slice() else {
        return Err(invalid(
            "exactly one verified attestation result is required",
        ));
    };
    let expected = Expected::new(image, workflow);
    let parsed = Parsed::new(wrapper)?;
    let mut violations = Vec::new();
    let caller_workflow_path = validate_statement(&expected, &parsed, &mut violations);
    validate_certificate(&expected, parsed.certificate, &mut violations);
    validate_bundle(parsed.wrapper, &bundle_bytes, &mut violations)?;
    if !violations.is_empty() {
        return Err(ToolError::many(Exit::ProvenanceMissing, violations));
    }
    Ok(OciVerifiedProvenance {
        predicate_type: PREDICATE_TYPE.to_owned(),
        subject_name: image.image_repository.clone(),
        subject_digest: image.output_digest.clone(),
        builder_id: workflow.builder_id.clone(),
        caller_workflow_path: caller_workflow_path.to_owned(),
        source_repository_uri: expected.source_uri,
        source_ref: workflow.r#ref.clone(),
        source_commit: image.source.commit_sha.clone(),
        invocation_id: expected.invocation,
        bundle_digest: crate::canon::digest_bytes(&bundle_bytes),
        verification_result_digest: crate::canon::digest_document(parsed.result)?,
    })
}

fn validate_statement<'a>(
    expected: &Expected<'_>,
    parsed: &'a Parsed<'_>,
    violations: &mut Vec<Violation>,
) -> &'a str {
    for (actual, wanted, label) in [
        (
            string(parsed.result, &["mediaType"]),
            VERIFICATION_MEDIA_TYPE,
            "verification result media type",
        ),
        (
            string(parsed.statement, &["_type"]),
            STATEMENT_TYPE,
            "statement type",
        ),
        (
            string(parsed.statement, &["predicateType"]),
            PREDICATE_TYPE,
            "predicate type",
        ),
        (
            string(parsed.build_definition, &["buildType"]),
            BUILD_TYPE,
            "build type",
        ),
        (
            string(parsed.workflow_parameter, &["repository"]),
            &expected.source_uri,
            "workflow repository",
        ),
        (
            string(parsed.workflow_parameter, &["ref"]),
            &expected.workflow.r#ref,
            "workflow ref",
        ),
        (
            string(parsed.github_parameters, &["event_name"]),
            "push",
            "workflow event",
        ),
        (
            string(parsed.github_parameters, &["runner_environment"]),
            "github-hosted",
            "runner authority",
        ),
        (
            string(parsed.run_details, &["builder", "id"]),
            &expected.workflow.builder_id,
            "SLSA builder id",
        ),
        (
            string(parsed.run_details, &["metadata", "invocationId"]),
            &expected.invocation,
            "SLSA invocation id",
        ),
    ] {
        check(actual, wanted, label, violations);
    }
    validate_subject_and_dependency(expected, parsed, violations);
    let path = string(parsed.workflow_parameter, &["path"]);
    if !path.starts_with(".github/workflows/")
        || !Path::new(path)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("yml"))
    {
        violations.push(violation(
            "caller workflow path is not a workflow YAML path",
        ));
    }
    if parsed.timestamps.is_empty() {
        violations.push(violation("verification result has no verified timestamp"));
    }
    path
}

fn validate_subject_and_dependency(
    expected: &Expected<'_>,
    parsed: &Parsed<'_>,
    violations: &mut Vec<Violation>,
) {
    if let [subject] = parsed.subjects {
        check(
            string(subject, &["name"]),
            &expected.image.image_repository,
            "subject name/unit authority",
            violations,
        );
        check(
            string(subject, &["digest", "sha256"]),
            expected.bare_digest,
            "subject digest",
            violations,
        );
    } else {
        violations.push(violation("statement must have exactly one subject"));
    }
    if let [dependency] = parsed.dependencies {
        check(
            string(dependency, &["uri"]),
            &expected.dependency,
            "source dependency URI",
            violations,
        );
        check(
            string(dependency, &["digest", "gitCommit"]),
            &expected.image.source.commit_sha,
            "source dependency digest",
            violations,
        );
    } else {
        violations.push(violation(
            "statement must have exactly one source dependency",
        ));
    }
}

fn validate_certificate(
    expected: &Expected<'_>,
    certificate: &Value,
    violations: &mut Vec<Violation>,
) {
    for (path, wanted, label) in [
        (
            "sourceRepositoryURI",
            expected.source_uri.as_str(),
            "certificate source repository",
        ),
        (
            "sourceRepositoryRef",
            expected.workflow.r#ref.as_str(),
            "certificate source ref",
        ),
        (
            "sourceRepositoryDigest",
            expected.image.source.commit_sha.as_str(),
            "certificate source digest",
        ),
        (
            "buildSignerURI",
            expected.workflow.builder_id.as_str(),
            "certificate reusable workflow",
        ),
        (
            "buildSignerDigest",
            expected.image.source.commit_sha.as_str(),
            "certificate reusable workflow digest",
        ),
        (
            "runInvocationURI",
            expected.invocation.as_str(),
            "certificate invocation",
        ),
        (
            "runnerEnvironment",
            "github-hosted",
            "certificate runner authority",
        ),
    ] {
        check(string(certificate, &[path]), wanted, label, violations);
    }
}

fn validate_bundle(
    wrapper: &Value,
    bundle_bytes: &[u8],
    violations: &mut Vec<Violation>,
) -> Result<()> {
    let supplied: Value = serde_json::from_slice(bundle_bytes).map_err(|error| {
        ToolError::single(
            Exit::ProvenanceMissing,
            "oci-provenance-bundle",
            format!("verified bundle JSON is invalid: {error}"),
        )
    })?;
    let verified = at(wrapper, &["attestation", "bundle"])?;
    if crate::canon::digest_document(verified)? != crate::canon::digest_document(&supplied)? {
        violations.push(violation(
            "the verification result does not contain the exact supplied bundle",
        ));
    }
    Ok(())
}

fn array<'a>(value: &'a Value, key: &str, message: &str) -> Result<&'a [Value]> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| invalid(message))
}

fn at<'a>(value: &'a Value, path: &[&str]) -> Result<&'a Value> {
    let mut current = value;
    for part in path {
        current = current
            .get(part)
            .ok_or_else(|| invalid(format!("verified provenance lacks `{}`", path.join("."))))?;
    }
    Ok(current)
}

fn string<'a>(value: &'a Value, path: &[&str]) -> &'a str {
    path.iter()
        .try_fold(value, |current, part| current.get(part))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn check(actual: &str, expected: &str, label: &str, violations: &mut Vec<Violation>) {
    if actual != expected {
        violations.push(violation(format!(
            "{label} `{actual}` does not match `{expected}`"
        )));
    }
}

fn violation(message: impl Into<String>) -> Violation {
    Violation::new("oci-provenance-binding", message)
}

fn invalid(message: impl Into<String>) -> ToolError {
    ToolError::single(Exit::ProvenanceMissing, "oci-provenance-binding", message)
}
