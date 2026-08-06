//! Canonical qualification-output and atomic-retention regression tests.

use std::fs;
use std::num::{NonZeroU32, NonZeroU64};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use aex_live_model_catalog::ProbeRun;
use aex_model_catalog::canonical::{NormalizedUsage, UsageCompleteness};
use aex_model_catalog::document::{Capability, PublisherId};
use aex_model_catalog::primitives::BoundedString;
use aex_model_catalog::receipt::{ConformanceReceipt, ObservedFact, ProbeId, ProbeOutcome};
use aex_wire::types::Timestamp;
use aex_wire::{ContentHash, to_jcs_bytes};

/// Test-root projection matching the production module's eventual crate path.
pub mod genesis {
    pub use aex_live_model_catalog::genesis::*;
}

/// Test-root projection matching the production module's eventual crate path.
pub mod tokenizer_oracle {
    pub use aex_live_model_catalog::tokenizer_oracle::*;
}

#[path = "../src/qualification_output.rs"]
mod qualification_output;

use qualification_output::{
    CHAT_TEMPLATE_IDENTITY_FACT, EVIDENCE_FILE, QualificationMetadata, QualificationOutputError,
    QualificationPlane, QualificationRegion, QualificationRepository, RECEIPT_FILE,
    ReviewedSourceSha, TOKENIZER_IDENTITY_FACT, build_qualification_outputs,
    deterministic_receipt_id, write_qualification_outputs,
    write_qualification_outputs_with_commit_failure,
};
use tokenizer_oracle::{
    EIGHTY_PERCENT_CORPUS_DIGEST, PINNED_CHAT_TEMPLATE_DIGEST, PINNED_TOKENIZER_DIGEST,
    WINDOW_PLUS_ONE_CORPUS_DIGEST,
};

const RAN_AT_MS: i64 = 1_800_000_000_000;
const SOURCE_SHA: &str = "1234567890abcdef1234567890abcdef12345678";
const MAXIMUM_COST_MICRO_USD: u64 = 2_450_000;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn bounded<const N: usize>(value: &str) -> BoundedString<N> {
    BoundedString::new(value).expect("test value is inside the reviewed bound")
}

fn fact(key: &str, value: &str) -> ObservedFact {
    ObservedFact {
        key: bounded(key),
        value: bounded(value),
    }
}

fn at(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("test timestamp is representable")
}

fn usage() -> NormalizedUsage {
    NormalizedUsage {
        input_tokens: 900_000,
        cache_read_input_tokens: 4_096,
        output_tokens: 16_384,
        provider_total_tokens: None,
        completeness: UsageCompleteness::Absent,
        ..NormalizedUsage::default()
    }
}

fn metadata() -> QualificationMetadata {
    QualificationMetadata {
        repository: QualificationRepository::AexhqAex,
        source_sha: ReviewedSourceSha::new(SOURCE_SHA).expect("full lowercase source SHA"),
        run_id: NonZeroU64::new(8_765_432).expect("non-zero run id"),
        run_attempt: NonZeroU32::new(2).expect("non-zero attempt"),
        ran_at: at(RAN_AT_MS),
        issued_at: at(RAN_AT_MS + 1_000),
        plane: QualificationPlane::Dev,
        region: QualificationRegion::EuWest1,
        publisher: PublisherId(bounded("aex-catalog-prd")),
        approved_budget_micro_usd: NonZeroU64::new(5_000_000).expect("non-zero approved budget"),
        approved_runtime_seconds: NonZeroU32::new(1_800).expect("non-zero approved runtime"),
    }
}

fn not_applicable(probe: ProbeId) -> Option<Capability> {
    match probe {
        ProbeId::P03 => Some(Capability::SystemInstruction),
        ProbeId::P04 | ProbeId::P06 => Some(Capability::Tools),
        ProbeId::P05 => Some(Capability::ParallelTools),
        ProbeId::P07 => Some(Capability::ToolChoiceRequired),
        ProbeId::P08 => Some(Capability::StructuredOutput),
        ProbeId::P09 => Some(Capability::Reasoning),
        ProbeId::P10 => Some(Capability::ReasoningReplay),
        ProbeId::P12 => Some(Capability::PromptCacheImplicit),
        ProbeId::P14 => Some(Capability::StopSequences),
        ProbeId::P01
        | ProbeId::P02
        | ProbeId::P11
        | ProbeId::P13
        | ProbeId::P15
        | ProbeId::P16
        | ProbeId::P17
        | ProbeId::P18
        | ProbeId::P19
        | ProbeId::P20
        | ProbeId::P21
        | ProbeId::P22
        | ProbeId::P23 => None,
    }
}

fn observations(probe: ProbeId) -> Vec<ObservedFact> {
    let tokenizer = PINNED_TOKENIZER_DIGEST.to_wire();
    let template = PINNED_CHAT_TEMPLATE_DIGEST.to_wire();
    let eighty_percent_corpus = EIGHTY_PERCENT_CORPUS_DIGEST.to_wire();
    let plus_one_corpus = WINDOW_PLUS_ONE_CORPUS_DIGEST.to_wire();
    match probe {
        ProbeId::P01 => vec![fact("text_bytes", "2")],
        ProbeId::P02 => vec![fact("output_bytes", "8192"), fact("frames", "200")],
        ProbeId::P15 => vec![fact("response_bytes", "1")],
        ProbeId::P16 => vec![
            fact("oracle_prompt_tokens", "800000"),
            fact("provider_prompt_tokens", "800000"),
            fact("context_window_tokens", "1000000"),
            fact(TOKENIZER_IDENTITY_FACT, &tokenizer),
            fact(CHAT_TEMPLATE_IDENTITY_FACT, &template),
            fact(
                qualification_output::CORPUS_IDENTITY_FACT,
                &eighty_percent_corpus,
            ),
        ],
        ProbeId::P17 => vec![
            fact("attempted_prompt_tokens", "1000001"),
            fact("context_window_tokens", "1000000"),
            fact("failure_kind", "context_overflow"),
            fact(TOKENIZER_IDENTITY_FACT, &tokenizer),
            fact(CHAT_TEMPLATE_IDENTITY_FACT, &template),
            fact(qualification_output::CORPUS_IDENTITY_FACT, &plus_one_corpus),
        ],
        ProbeId::P19 => vec![fact("rate_limit_source", "not_provided")],
        ProbeId::P20 => vec![fact(
            "error_body_shapes",
            "openai_envelope,openai_envelope,openai_envelope",
        )],
        ProbeId::P21 => vec![fact("artifacts_scanned", "10")],
        ProbeId::P23 => vec![fact("idle_limit_ms", "60000")],
        ProbeId::P03
        | ProbeId::P04
        | ProbeId::P05
        | ProbeId::P06
        | ProbeId::P07
        | ProbeId::P08
        | ProbeId::P09
        | ProbeId::P10
        | ProbeId::P11
        | ProbeId::P12
        | ProbeId::P13
        | ProbeId::P14
        | ProbeId::P18
        | ProbeId::P22 => Vec::new(),
    }
}

fn complete_matrix() -> Vec<ProbeRun> {
    ProbeId::ALL
        .into_iter()
        .map(|probe| ProbeRun {
            probe,
            outcome: not_applicable(probe).map_or(ProbeOutcome::Pass, |capability| {
                ProbeOutcome::NotApplicable { capability }
            }),
            observed: observations(probe),
            duration_ms: 5,
        })
        .collect()
}

fn build() -> qualification_output::QualificationOutputs {
    build_qualification_outputs(
        complete_matrix(),
        usage(),
        MAXIMUM_COST_MICRO_USD,
        &metadata(),
    )
    .expect("complete, closed qualification earns canonical outputs")
}

struct TempWorkspace(PathBuf);

impl TempWorkspace {
    fn new() -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "aex-qualification-output-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create isolated test workspace");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn evidence_is_canonical_redacted_and_precedes_receipt_identity() {
    let metadata = metadata();
    let expected_id = deterministic_receipt_id(&metadata).expect("deterministic UUIDv7");
    let outputs = build_qualification_outputs(
        complete_matrix(),
        usage(),
        MAXIMUM_COST_MICRO_USD,
        &metadata,
    )
    .expect("complete qualification output");
    let evidence: serde_json::Value =
        serde_json::from_slice(outputs.evidence()).expect("evidence JSON");
    let receipt: ConformanceReceipt =
        serde_json::from_slice(outputs.receipt()).expect("receipt JSON");

    assert_eq!(
        outputs.evidence(),
        to_jcs_bytes(&evidence).expect("canonical evidence")
    );
    assert_eq!(
        outputs.receipt(),
        to_jcs_bytes(&receipt).expect("canonical receipt")
    );
    assert_eq!(outputs.evidence_digest, ContentHash::of(outputs.evidence()));
    assert_eq!(receipt.evidence_digest, outputs.evidence_digest);
    assert_eq!(outputs.receipt_id, expected_id);
    assert_eq!(receipt.receipt_id, expected_id);
    assert_eq!(receipt.tokens_spent, usage());
    assert_eq!(evidence["source"]["repository"], "aexhq/aex");
    assert_eq!(evidence["source"]["sha"], SOURCE_SHA);
    assert_eq!(evidence["run"]["plane"], "dev");
    assert_eq!(evidence["run"]["region"], "eu-west-1");
    assert_eq!(
        evidence["bounds"]["maximum_cost_micro_usd"],
        MAXIMUM_COST_MICRO_USD
    );
    assert_eq!(evidence["aggregate_usage"]["input_tokens"], 900_000);
    assert_eq!(
        evidence["aggregate_usage"]["completeness"]["kind"],
        "absent"
    );
    assert!(evidence["aggregate_usage"]["provider_total_tokens"].is_null());
    assert_eq!(
        evidence["authority"]["tokenizer_sha256"],
        PINNED_TOKENIZER_DIGEST.to_wire()
    );

    let rendered = String::from_utf8(outputs.evidence().to_vec()).expect("evidence UTF-8");
    for forbidden in [
        "catalog_entry_digest",
        "evidence_digest",
        "receipt_id",
        "catalog_document",
        "provider-secret-text",
    ] {
        assert!(!rendered.contains(forbidden), "evidence leaked {forbidden}");
    }
}

#[test]
fn receipt_uuid_is_stable_and_changes_with_immutable_run_identity() {
    let original = metadata();
    let first = deterministic_receipt_id(&original).expect("first receipt id");
    let second = deterministic_receipt_id(&original).expect("same receipt id");
    assert_eq!(first, second);
    assert_eq!(
        first.unix_millis(),
        u64::try_from(RAN_AT_MS).expect("positive time")
    );

    let mut another_run = original;
    another_run.run_id = NonZeroU64::new(8_765_433).expect("non-zero run id");
    assert_ne!(
        first,
        deterministic_receipt_id(&another_run).expect("different run receipt id")
    );
}

#[test]
fn matrix_and_fact_input_order_cannot_change_any_canonical_output() {
    let expected = build();
    let mut reordered = complete_matrix();
    reordered.reverse();
    for run in &mut reordered {
        run.observed.reverse();
    }
    let actual =
        build_qualification_outputs(reordered, usage(), MAXIMUM_COST_MICRO_USD, &metadata())
            .expect("semantic input permutation remains deterministic");
    assert_eq!(actual, expected);
}

#[test]
fn provider_text_and_unreviewed_facts_cannot_enter_outputs() {
    let mut failed = complete_matrix();
    let optional = failed
        .iter_mut()
        .find(|run| run.probe == ProbeId::P03)
        .expect("P-03 row");
    optional.outcome = ProbeOutcome::Fail {
        detail: bounded("provider-secret-text"),
    };
    assert!(matches!(
        build_qualification_outputs(failed, usage(), MAXIMUM_COST_MICRO_USD, &metadata()),
        Err(QualificationOutputError::FailedProbe {
            probe: ProbeId::P03
        })
    ));

    let mut arbitrary = complete_matrix();
    arbitrary[0]
        .observed
        .push(fact("provider_response", "provider-secret-text"));
    assert!(matches!(
        build_qualification_outputs(arbitrary, usage(), MAXIMUM_COST_MICRO_USD, &metadata()),
        Err(QualificationOutputError::UnexpectedObservedFact { .. })
    ));
}

#[test]
fn both_long_context_rows_must_carry_exact_reviewed_oracle_identities() {
    let mut missing = complete_matrix();
    missing
        .iter_mut()
        .find(|run| run.probe == ProbeId::P17)
        .expect("P-17 row")
        .observed
        .retain(|fact| fact.key.as_str() != CHAT_TEMPLATE_IDENTITY_FACT);
    assert!(matches!(
        build_qualification_outputs(missing, usage(), MAXIMUM_COST_MICRO_USD, &metadata()),
        Err(QualificationOutputError::MissingObservedFact {
            probe: ProbeId::P17,
            key: CHAT_TEMPLATE_IDENTITY_FACT
        })
    ));

    let mut wrong = complete_matrix();
    wrong
        .iter_mut()
        .find(|run| run.probe == ProbeId::P16)
        .expect("P-16 row")
        .observed
        .iter_mut()
        .find(|fact| fact.key.as_str() == qualification_output::CORPUS_IDENTITY_FACT)
        .expect("corpus fact")
        .value = bounded(&format!("sha256:{}", "0".repeat(64)));
    assert!(matches!(
        build_qualification_outputs(wrong, usage(), MAXIMUM_COST_MICRO_USD, &metadata()),
        Err(QualificationOutputError::InvalidObservedFact {
            probe: ProbeId::P16,
            ..
        })
    ));
}

#[test]
fn approved_bounds_and_source_identity_are_fail_closed() {
    assert!(ReviewedSourceSha::new(&"A".repeat(40)).is_err());
    assert!(ReviewedSourceSha::new("main").is_err());

    let mut too_small = metadata();
    too_small.approved_budget_micro_usd = NonZeroU64::new(1).expect("non-zero budget");
    assert!(matches!(
        build_qualification_outputs(
            complete_matrix(),
            usage(),
            MAXIMUM_COST_MICRO_USD,
            &too_small
        ),
        Err(QualificationOutputError::CostOutsideBudget { .. })
    ));

    let mut too_long = metadata();
    too_long.approved_runtime_seconds = NonZeroU32::new(3_601).expect("non-zero runtime");
    assert!(matches!(
        build_qualification_outputs(
            complete_matrix(),
            usage(),
            MAXIMUM_COST_MICRO_USD,
            &too_long,
        ),
        Err(QualificationOutputError::BoundOutsideCeiling {
            field: "approved_runtime_seconds",
            ..
        })
    ));
}

#[test]
fn writer_commits_only_the_exact_two_assurance_files() {
    let workspace = TempWorkspace::new();
    let outputs = build();
    let directory = write_qualification_outputs(workspace.path(), &outputs)
        .expect("commit exact qualification outputs");
    let mut names = fs::read_dir(&directory)
        .expect("read output directory")
        .map(|entry| {
            entry
                .expect("read output entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(names, vec![EVIDENCE_FILE, RECEIPT_FILE]);
    assert_eq!(
        fs::read(directory.join(EVIDENCE_FILE)).expect("evidence bytes"),
        outputs.evidence()
    );
    assert_eq!(
        fs::read(directory.join(RECEIPT_FILE)).expect("receipt bytes"),
        outputs.receipt()
    );
}

#[test]
fn writer_rolls_back_every_temp_and_final_after_commit_failure() {
    let workspace = TempWorkspace::new();
    let outputs = build();
    assert!(matches!(
        write_qualification_outputs_with_commit_failure(workspace.path(), &outputs),
        Err(QualificationOutputError::InjectedCommitFailure)
    ));
    let parent = workspace.path().join(".tmp/model-catalog");
    let directory = parent.join("qualification");
    assert!(!directory.exists());
    assert_eq!(
        fs::read_dir(parent)
            .expect("qualification parent remains readable")
            .count(),
        0
    );
}

#[test]
fn writer_never_replaces_or_cleans_preexisting_content() {
    let workspace = TempWorkspace::new();
    let directory = workspace
        .path()
        .join(qualification_output::QUALIFICATION_DIRECTORY);
    fs::create_dir_all(&directory).expect("create qualification directory");
    let sentinel = directory.join("owner-review.txt");
    fs::write(&sentinel, b"preserve me").expect("write sentinel");
    assert!(matches!(
        write_qualification_outputs(workspace.path(), &build()),
        Err(QualificationOutputError::OutputAlreadyExists { .. })
    ));
    assert_eq!(
        fs::read(sentinel).expect("sentinel survives"),
        b"preserve me"
    );
}
