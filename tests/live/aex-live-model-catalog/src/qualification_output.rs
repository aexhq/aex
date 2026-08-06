//! Canonical, redacted outputs for one completed protected qualification run.
//!
//! Evidence is canonicalized and hashed before receipt construction. It never
//! contains a receipt, either digest, provider response text, or credentials,
//! so the evidence-to-receipt relationship cannot cycle.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::num::{NonZeroU32, NonZeroU64};
use std::path::{Path, PathBuf};

use aex_model_catalog::canonical::NormalizedUsage;
use aex_model_catalog::document::{Capability, PublisherId};
use aex_model_catalog::primitives::BoundedString;
use aex_model_catalog::receipt::{ObservedFact, ProbeId, ProbeOutcome};
use aex_wire::canonical::CanonicalError;
use aex_wire::types::Timestamp;
use aex_wire::{ContentHash, Uuid7, to_jcs_bytes};
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use crate::ProbeRun;
use crate::genesis::{GenesisError, GenesisMetadata, build_deepseek_genesis};
use crate::tokenizer_oracle::{
    EIGHTY_PERCENT_CORPUS_DIGEST, PINNED_CHAT_TEMPLATE_DIGEST, PINNED_TOKENIZER_DIGEST,
    WINDOW_PLUS_ONE_CORPUS_DIGEST,
};

/// Workspace-relative directory which alone may receive qualification outputs.
pub const QUALIFICATION_DIRECTORY: &str = ".tmp/model-catalog/qualification";
/// Canonical redacted evidence filename.
pub const EVIDENCE_FILE: &str = "evidence.json";
/// Canonical conformance receipt filename.
pub const RECEIPT_FILE: &str = "receipt.json";
/// Receipt fact key carrying the reviewed tokenizer identity.
pub const TOKENIZER_IDENTITY_FACT: &str = "tokenizer_sha256";
/// Receipt fact key carrying the reviewed chat-template identity.
pub const CHAT_TEMPLATE_IDENTITY_FACT: &str = "chat_template_sha256";
/// Receipt fact key carrying the exact long-context corpus identity.
pub const CORPUS_IDENTITY_FACT: &str = "corpus_sha256";

const EVIDENCE_SCHEMA: &str = "aex.model-catalog-qualification-evidence.v1";
const REPOSITORY: &str = "aexhq/aex";
const PROVIDER: &str = "deepseek";
const MODEL: &str = "deepseek-v4-flash";
const MAXIMUM_APPROVED_BUDGET_MICRO_USD: u64 = 5_000_000;
const MAXIMUM_APPROVED_RUNTIME_SECONDS: u32 = 3_600;
const RECEIPT_ID_DOMAIN: &[u8] = b"aex.model-catalog-qualification-receipt.v1\0";

/// The only repository which may earn the public model-catalog authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualificationRepository {
    /// Public `aexhq/aex` repository.
    AexhqAex,
}

impl QualificationRepository {
    const fn as_str(self) -> &'static str {
        match self {
            Self::AexhqAex => REPOSITORY,
        }
    }
}

/// The only plane admitted by the genesis qualification lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualificationPlane {
    /// Development qualification plane.
    Dev,
}

impl QualificationPlane {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "dev",
        }
    }
}

/// The only region admitted by the genesis qualification lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualificationRegion {
    /// Ireland, matching the launch AWS stack.
    EuWest1,
}

impl QualificationRegion {
    const fn as_str(self) -> &'static str {
        match self {
            Self::EuWest1 => "eu-west-1",
        }
    }
}

/// Validated lowercase Git source identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewedSourceSha(String);

impl ReviewedSourceSha {
    /// Validates one exact lowercase 40-hex Git commit identity.
    ///
    /// # Errors
    ///
    /// Returns [`QualificationOutputError::InvalidSourceSha`] for any symbolic,
    /// uppercase, abbreviated, or malformed value.
    pub fn new(value: &str) -> Result<Self, QualificationOutputError> {
        if value.len() != 40
            || !value
                .as_bytes()
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        {
            return Err(QualificationOutputError::InvalidSourceSha);
        }
        Ok(Self(value.to_owned()))
    }

    /// Exact lowercase commit identity.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Explicit authority and bounds supplied by the protected workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualificationMetadata {
    /// Fixed public repository identity.
    pub repository: QualificationRepository,
    /// Exact source commit which ran the qualifier.
    pub source_sha: ReviewedSourceSha,
    /// GitHub Actions run id.
    pub run_id: NonZeroU64,
    /// GitHub Actions run attempt.
    pub run_attempt: NonZeroU32,
    /// Start time of the completed matrix.
    pub ran_at: Timestamp,
    /// Time at which the candidate document is cut.
    pub issued_at: Timestamp,
    /// Fixed development plane.
    pub plane: QualificationPlane,
    /// Fixed launch region.
    pub region: QualificationRegion,
    /// Stable catalog publisher id.
    pub publisher: PublisherId,
    /// Human-approved upper spend bound.
    pub approved_budget_micro_usd: NonZeroU64,
    /// Human-approved whole-run deadline.
    pub approved_runtime_seconds: NonZeroU32,
}

/// Canonical bytes earned by one complete qualification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualificationOutputs {
    evidence: Vec<u8>,
    receipt: Vec<u8>,
    /// SHA-256 identity passed into the standalone receipt.
    pub evidence_digest: ContentHash,
    /// Deterministic time-ordered receipt identity.
    pub receipt_id: Uuid7,
}

impl QualificationOutputs {
    /// Canonical redacted evidence bytes.
    #[must_use]
    pub fn evidence(&self) -> &[u8] {
        &self.evidence
    }

    /// Canonical conformance-receipt bytes.
    #[must_use]
    pub fn receipt(&self) -> &[u8] {
        &self.receipt
    }
}

/// Why canonical qualification output could not be earned or retained.
#[derive(Debug, thiserror::Error)]
pub enum QualificationOutputError {
    /// Source identity was not exact lowercase full-length hex.
    #[error("qualification source SHA must be exactly 40 lowercase hexadecimal characters")]
    InvalidSourceSha,
    /// The admitted cost is zero or exceeds the human-approved budget.
    #[error("maximum cost {maximum} micro-USD is outside approved budget {approved} micro-USD")]
    CostOutsideBudget {
        /// Deterministic preflight cost ceiling.
        maximum: u64,
        /// Protected workflow approval ceiling.
        approved: u64,
    },
    /// A protected bound exceeds the closed workflow ceiling.
    #[error("qualification {field} {value} exceeds its closed ceiling")]
    BoundOutsideCeiling {
        /// Bound which was rejected.
        field: &'static str,
        /// Supplied value.
        value: u64,
    },
    /// A failed row could carry arbitrary detail and is never an output input.
    #[error("probe {probe:?} failed; canonical authority output requires pass or not-applicable")]
    FailedProbe {
        /// Failed probe.
        probe: ProbeId,
    },
    /// More than one row claimed the same probe identity.
    #[error("qualification output contains duplicate probe {probe:?}")]
    DuplicateProbe {
        /// Duplicated probe.
        probe: ProbeId,
    },
    /// A fact key is not part of the closed redacted vocabulary for its probe.
    #[error("probe {probe:?} supplied non-authoritative observed fact `{key}`")]
    UnexpectedObservedFact {
        /// Probe carrying the fact.
        probe: ProbeId,
        /// Rejected key.
        key: String,
    },
    /// A probe supplied the same fact key more than once.
    #[error("probe {probe:?} supplied duplicate observed fact `{key}`")]
    DuplicateObservedFact {
        /// Probe carrying the duplicate.
        probe: ProbeId,
        /// Duplicated key.
        key: String,
    },
    /// A closed observed fact has a malformed or unreviewed value.
    #[error("probe {probe:?} supplied invalid value for observed fact `{key}`")]
    InvalidObservedFact {
        /// Probe carrying the fact.
        probe: ProbeId,
        /// Invalid key.
        key: String,
    },
    /// P-16 or P-17 did not carry one required closed observation.
    #[error("probe {probe:?} is missing required observed fact `{key}`")]
    MissingObservedFact {
        /// Long-context probe missing the observation.
        probe: ProbeId,
        /// Required identity key.
        key: &'static str,
    },
    /// `ran_at` cannot be encoded by `UUIDv7`'s 48-bit timestamp field.
    #[error("qualification ran_at is outside the UUIDv7 timestamp range")]
    ReceiptTimestampOutOfRange,
    /// Canonical evidence serialization failed.
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
    /// The standalone assurance builder rejected the completed matrix.
    #[error(transparent)]
    Genesis(#[from] GenesisError),
    /// The final qualification path already exists and is never replaced.
    #[error("qualification output path already exists: {path}")]
    OutputAlreadyExists {
        /// Fixed output directory.
        path: PathBuf,
    },
    /// A filesystem operation failed while staging or committing outputs.
    #[error("could not {operation} canonical qualification outputs: {source}")]
    Io {
        /// Fixed operation label without path or secret data.
        operation: &'static str,
        /// Filesystem failure.
        #[source]
        source: io::Error,
    },
    /// A fixed child of the workspace was a symlink or non-directory.
    #[error("qualification output parent is not a real directory: {path}")]
    UnsafeOutputParent {
        /// Rejected workspace child.
        path: PathBuf,
    },
    /// Test-only interruption before the directory commit used to prove rollback.
    #[cfg(test)]
    #[error("injected qualification-output directory-commit failure")]
    InjectedCommitFailure,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct QualificationEvidence<'a> {
    schema: &'static str,
    target: EvidenceTarget,
    source: EvidenceSource<'a>,
    run: EvidenceRun,
    authority: EvidenceAuthority<'a>,
    bounds: EvidenceBounds,
    aggregate_usage: NormalizedUsage,
    probes: Vec<EvidenceProbe>,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct EvidenceTarget {
    provider: &'static str,
    model: &'static str,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct EvidenceSource<'a> {
    repository: &'static str,
    sha: &'a str,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct EvidenceRun {
    id: u64,
    attempt: u32,
    ran_at: Timestamp,
    issued_at: Timestamp,
    plane: &'static str,
    region: &'static str,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct EvidenceAuthority<'a> {
    publisher: &'a PublisherId,
    tokenizer_sha256: ContentHash,
    chat_template_sha256: ContentHash,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct EvidenceBounds {
    approved_budget_micro_usd: u64,
    maximum_cost_micro_usd: u64,
    approved_runtime_seconds: u32,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct EvidenceProbe {
    probe: ProbeId,
    outcome: RedactedOutcome,
    observed: Vec<ObservedFact>,
    duration_ms: u32,
}

#[derive(Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
enum RedactedOutcome {
    Pass,
    NotApplicable { capability: Capability },
}

/// Produces evidence first, hashes those exact bytes, and only then constructs
/// a canonical standalone conformance receipt.
///
/// # Errors
///
/// Rejects incomplete/failed/duplicate matrices, unreviewed facts, missing
/// tokenizer identities, invalid bounds or metadata, canonicalization failure,
/// and every standalone assurance invariant.
pub fn build_qualification_outputs(
    mut runs: Vec<ProbeRun>,
    aggregate_usage: NormalizedUsage,
    maximum_cost_micro_usd: u64,
    metadata: &QualificationMetadata,
) -> Result<QualificationOutputs, QualificationOutputError> {
    validate_bounds(maximum_cost_micro_usd, metadata)?;
    let receipt_id = deterministic_receipt_id(metadata)?;
    runs.sort_by_key(|run| run.probe);
    for run in &mut runs {
        run.observed.sort_by(|left, right| {
            (left.key.as_str(), left.value.as_str())
                .cmp(&(right.key.as_str(), right.value.as_str()))
        });
    }
    let probes = redact_probes(&runs)?;
    require_long_context_facts(&probes)?;
    let evidence = QualificationEvidence {
        schema: EVIDENCE_SCHEMA,
        target: EvidenceTarget {
            provider: PROVIDER,
            model: MODEL,
        },
        source: EvidenceSource {
            repository: metadata.repository.as_str(),
            sha: metadata.source_sha.as_str(),
        },
        run: EvidenceRun {
            id: metadata.run_id.get(),
            attempt: metadata.run_attempt.get(),
            ran_at: metadata.ran_at,
            issued_at: metadata.issued_at,
            plane: metadata.plane.as_str(),
            region: metadata.region.as_str(),
        },
        authority: EvidenceAuthority {
            publisher: &metadata.publisher,
            tokenizer_sha256: PINNED_TOKENIZER_DIGEST,
            chat_template_sha256: PINNED_CHAT_TEMPLATE_DIGEST,
        },
        bounds: EvidenceBounds {
            approved_budget_micro_usd: metadata.approved_budget_micro_usd.get(),
            maximum_cost_micro_usd,
            approved_runtime_seconds: metadata.approved_runtime_seconds.get(),
        },
        aggregate_usage,
        probes,
    };
    let canonical_evidence = to_jcs_bytes(&evidence)?;
    let evidence_digest = ContentHash::of(&canonical_evidence);
    let genesis = build_deepseek_genesis(
        runs,
        GenesisMetadata {
            receipt_id,
            plane: aex_model_catalog::receipt::PlaneId(BoundedString::truncating(
                metadata.plane.as_str(),
            )),
            region: aex_model_catalog::receipt::Region(BoundedString::truncating(
                metadata.region.as_str(),
            )),
            ran_at: metadata.ran_at,
            provider_request_ids: Vec::new(),
            tokens_spent: aggregate_usage,
            evidence_digest,
        },
    )?;
    Ok(QualificationOutputs {
        evidence: canonical_evidence,
        receipt: genesis.canonical_receipt,
        evidence_digest,
        receipt_id,
    })
}

/// Derives `UUIDv7` entropy from the immutable repository/source/run identity.
///
/// # Errors
///
/// Returns [`QualificationOutputError::ReceiptTimestampOutOfRange`] when
/// `ran_at` is negative or exceeds `UUIDv7`'s 48-bit millisecond field.
pub fn deterministic_receipt_id(
    metadata: &QualificationMetadata,
) -> Result<Uuid7, QualificationOutputError> {
    let millis = u64::try_from(metadata.ran_at.unix_millis())
        .ok()
        .filter(|value| *value < (1_u64 << 48))
        .ok_or(QualificationOutputError::ReceiptTimestampOutOfRange)?;
    let mut hash = Sha256::new();
    hash.update(RECEIPT_ID_DOMAIN);
    hash.update(metadata.repository.as_str().as_bytes());
    hash.update([0]);
    hash.update(metadata.source_sha.as_str().as_bytes());
    hash.update([0]);
    hash.update(metadata.run_id.get().to_be_bytes());
    hash.update(metadata.run_attempt.get().to_be_bytes());
    let digest = hash.finalize();
    let mut entropy = [0_u8; 10];
    entropy.copy_from_slice(&digest[..10]);
    Ok(Uuid7::compose(millis, entropy))
}

fn validate_bounds(
    maximum_cost_micro_usd: u64,
    metadata: &QualificationMetadata,
) -> Result<(), QualificationOutputError> {
    let approved = metadata.approved_budget_micro_usd.get();
    if maximum_cost_micro_usd == 0 || maximum_cost_micro_usd > approved {
        return Err(QualificationOutputError::CostOutsideBudget {
            maximum: maximum_cost_micro_usd,
            approved,
        });
    }
    if approved > MAXIMUM_APPROVED_BUDGET_MICRO_USD {
        return Err(QualificationOutputError::BoundOutsideCeiling {
            field: "approved_budget_micro_usd",
            value: approved,
        });
    }
    let runtime = metadata.approved_runtime_seconds.get();
    if runtime > MAXIMUM_APPROVED_RUNTIME_SECONDS {
        return Err(QualificationOutputError::BoundOutsideCeiling {
            field: "approved_runtime_seconds",
            value: u64::from(runtime),
        });
    }
    Ok(())
}

fn redact_probes(runs: &[ProbeRun]) -> Result<Vec<EvidenceProbe>, QualificationOutputError> {
    let mut sorted = runs.to_vec();
    sorted.sort_by_key(|run| run.probe);
    let mut seen = BTreeSet::new();
    let mut output = Vec::with_capacity(sorted.len());
    for run in sorted {
        if !seen.insert(run.probe) {
            return Err(QualificationOutputError::DuplicateProbe { probe: run.probe });
        }
        let outcome = match run.outcome {
            ProbeOutcome::Pass => RedactedOutcome::Pass,
            ProbeOutcome::NotApplicable { capability } => {
                RedactedOutcome::NotApplicable { capability }
            }
            ProbeOutcome::Fail { .. } => {
                return Err(QualificationOutputError::FailedProbe { probe: run.probe });
            }
        };
        let observed = validate_observed(run.probe, run.observed)?;
        output.push(EvidenceProbe {
            probe: run.probe,
            outcome,
            observed,
            duration_ms: run.duration_ms,
        });
    }
    Ok(output)
}

fn validate_observed(
    probe: ProbeId,
    mut facts: Vec<ObservedFact>,
) -> Result<Vec<ObservedFact>, QualificationOutputError> {
    facts.sort_by(|left, right| {
        (left.key.as_str(), left.value.as_str()).cmp(&(right.key.as_str(), right.value.as_str()))
    });
    let mut seen = BTreeSet::new();
    for fact in &facts {
        let key = fact.key.as_str();
        if !allowed_fact_keys(probe).contains(&key) {
            return Err(QualificationOutputError::UnexpectedObservedFact {
                probe,
                key: key.to_owned(),
            });
        }
        if !seen.insert(key) {
            return Err(QualificationOutputError::DuplicateObservedFact {
                probe,
                key: key.to_owned(),
            });
        }
        if !valid_fact_value(probe, key, fact.value.as_str()) {
            return Err(QualificationOutputError::InvalidObservedFact {
                probe,
                key: key.to_owned(),
            });
        }
    }
    Ok(facts)
}

const fn allowed_fact_keys(probe: ProbeId) -> &'static [&'static str] {
    match probe {
        ProbeId::P01 => &["text_bytes"],
        ProbeId::P02 => &["frames", "output_bytes"],
        ProbeId::P04 | ProbeId::P05 => &["tool_calls"],
        ProbeId::P07 => &["tool_choice_modes"],
        ProbeId::P09 => &["reasoning_tokens"],
        ProbeId::P12 => &["cache_read_tokens"],
        ProbeId::P15 => &["response_bytes"],
        ProbeId::P16 => &[
            CHAT_TEMPLATE_IDENTITY_FACT,
            "context_window_tokens",
            CORPUS_IDENTITY_FACT,
            "oracle_prompt_tokens",
            "provider_prompt_tokens",
            TOKENIZER_IDENTITY_FACT,
        ],
        ProbeId::P17 => &[
            "attempted_prompt_tokens",
            CHAT_TEMPLATE_IDENTITY_FACT,
            "context_window_tokens",
            CORPUS_IDENTITY_FACT,
            "failure_kind",
            TOKENIZER_IDENTITY_FACT,
        ],
        ProbeId::P19 => &["rate_limit_source"],
        ProbeId::P20 => &["error_body_shapes"],
        ProbeId::P21 => &["artifacts_scanned"],
        ProbeId::P23 => &["idle_limit_ms"],
        ProbeId::P03
        | ProbeId::P06
        | ProbeId::P08
        | ProbeId::P10
        | ProbeId::P11
        | ProbeId::P13
        | ProbeId::P14
        | ProbeId::P18
        | ProbeId::P22 => &[],
    }
}

fn valid_fact_value(probe: ProbeId, key: &str, value: &str) -> bool {
    match key {
        TOKENIZER_IDENTITY_FACT => value == PINNED_TOKENIZER_DIGEST.to_wire(),
        CHAT_TEMPLATE_IDENTITY_FACT => value == PINNED_CHAT_TEMPLATE_DIGEST.to_wire(),
        CORPUS_IDENTITY_FACT => match probe {
            ProbeId::P16 => value == EIGHTY_PERCENT_CORPUS_DIGEST.to_wire(),
            ProbeId::P17 => value == WINDOW_PLUS_ONE_CORPUS_DIGEST.to_wire(),
            _ => false,
        },
        "oracle_prompt_tokens" | "provider_prompt_tokens" => {
            probe == ProbeId::P16 && value == "800000"
        }
        "attempted_prompt_tokens" => probe == ProbeId::P17 && value == "1000001",
        "context_window_tokens" => {
            matches!(probe, ProbeId::P16 | ProbeId::P17) && value == "1000000"
        }
        "failure_kind" => probe == ProbeId::P17 && value == "context_overflow",
        "rate_limit_source" => matches!(
            value,
            "not_provided" | "retry_after_header" | "vendor_headers" | "error_body"
        ),
        "error_body_shapes" => {
            let values = value.split(',').collect::<Vec<_>>();
            values.len() == 3
                && values.iter().all(|shape| {
                    matches!(
                        *shape,
                        "openai_envelope"
                            | "anthropic_envelope"
                            | "google_snake_case"
                            | "google_rpc"
                            | "status_only"
                            | "other_redacted"
                    )
                })
        }
        _ => value
            .parse::<u64>()
            .is_ok_and(|number| number.to_string() == value),
    }
}

fn require_long_context_facts(probes: &[EvidenceProbe]) -> Result<(), QualificationOutputError> {
    for (probe, keys) in [
        (ProbeId::P16, allowed_fact_keys(ProbeId::P16)),
        (ProbeId::P17, allowed_fact_keys(ProbeId::P17)),
    ] {
        let row = probes.iter().find(|row| row.probe == probe);
        for key in keys {
            if !row.is_some_and(|row| row.observed.iter().any(|fact| fact.key.as_str() == *key)) {
                return Err(QualificationOutputError::MissingObservedFact { probe, key });
            }
        }
    }
    Ok(())
}

/// Atomically retains the exact two files below the fixed qualification
/// directory of `workspace_root`.
///
/// All files are written and synced in a sibling staging directory. One final
/// directory rename makes both visible together. A failure removes only
/// the staging directory owned by this call; an existing final path is never
/// replaced or removed.
///
/// # Errors
///
/// Refuses any pre-existing final qualification path and reports filesystem
/// failures.
pub fn write_qualification_outputs(
    workspace_root: &Path,
    outputs: &QualificationOutputs,
) -> Result<PathBuf, QualificationOutputError> {
    write_outputs(workspace_root, outputs, false)
}

#[cfg(test)]
#[allow(
    dead_code,
    reason = "called by the path-mounted integration test; unused by the library unit-test harness"
)]
pub(crate) fn write_qualification_outputs_with_commit_failure(
    workspace_root: &Path,
    outputs: &QualificationOutputs,
) -> Result<PathBuf, QualificationOutputError> {
    write_outputs(workspace_root, outputs, true)
}

fn write_outputs(
    workspace_root: &Path,
    outputs: &QualificationOutputs,
    fail_before_commit: bool,
) -> Result<PathBuf, QualificationOutputError> {
    let parent = workspace_root.join(".tmp/model-catalog");
    let directory = parent.join("qualification");
    fs::create_dir_all(&parent).map_err(|source| io_error("create output parent", source))?;
    for path in [workspace_root.join(".tmp"), parent.clone()] {
        let metadata = fs::symlink_metadata(&path)
            .map_err(|source| io_error("inspect output parent", source))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(QualificationOutputError::UnsafeOutputParent { path });
        }
    }
    if path_exists_no_follow(&directory)? {
        return Err(QualificationOutputError::OutputAlreadyExists { path: directory });
    }
    let files = [
        (EVIDENCE_FILE, outputs.evidence()),
        (RECEIPT_FILE, outputs.receipt()),
    ];
    let staging = parent.join(format!(
        ".qualification-{}.tmp",
        hex::encode(outputs.receipt_id.as_bytes())
    ));
    fs::create_dir(&staging).map_err(|source| io_error("create staging directory", source))?;
    let mut committed = false;
    let result = (|| {
        for (name, bytes) in files {
            let mut file = create_new(&staging.join(name))?;
            file.write_all(bytes)
                .map_err(|source| io_error("write staged output", source))?;
            file.sync_all()
                .map_err(|source| io_error("sync staged output", source))?;
        }
        sync_directory(&staging)?;
        #[cfg(test)]
        if fail_before_commit {
            return Err(QualificationOutputError::InjectedCommitFailure);
        }
        #[cfg(not(test))]
        let _ = fail_before_commit;
        fs::rename(&staging, &directory)
            .map_err(|source| io_error("commit staging directory", source))?;
        committed = true;
        sync_directory(&parent)?;
        Ok(())
    })();
    if let Err(error) = result {
        if committed {
            let _ = fs::remove_dir_all(&directory);
            let _ = sync_directory(&parent);
        } else {
            let _ = fs::remove_dir_all(&staging);
        }
        return Err(error);
    }
    Ok(directory)
}

fn create_new(path: &Path) -> Result<File, QualificationOutputError> {
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|source| io_error("create staged output", source))
}

fn io_error(operation: &'static str, source: io::Error) -> QualificationOutputError {
    QualificationOutputError::Io { operation, source }
}

fn path_exists_no_follow(path: &Path) -> Result<bool, QualificationOutputError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(io_error("inspect final output path", source)),
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), QualificationOutputError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| io_error("sync output directory", source))
}

#[cfg(not(unix))]
#[allow(
    clippy::unnecessary_wraps,
    reason = "the no-op platform branch preserves one fallible cross-platform call site"
)]
fn sync_directory(_path: &Path) -> Result<(), QualificationOutputError> {
    Ok(())
}
