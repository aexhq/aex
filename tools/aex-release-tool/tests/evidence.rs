//! Promotion admission: one fixture per numbered rule, each asserting the exit
//! code that classifies it.

mod common;

use std::collections::BTreeMap;

use aex_release_tool::admit::{AdmissionInputs, OperationalReadiness, Plane, admit};
use aex_release_tool::artifact::ArtifactEnvelope;
use aex_release_tool::evidence::{FreshnessPolicy, Receipt};
use aex_release_tool::manifest::CompositionManifest;
use aex_release_tool::verification::VerificationStatement;
use common::docs::{
    BUILDER, digest, valid_envelope, valid_manifest, valid_receipt, valid_statement,
};

const UNIT: &str = "regional-session-api";

fn manifest() -> CompositionManifest {
    serde_json::from_value::<CompositionManifest>(valid_manifest())
        .expect("fixture manifest")
        .seal()
        .expect("a sealed manifest")
}

fn envelopes() -> BTreeMap<String, ArtifactEnvelope> {
    let envelope: ArtifactEnvelope =
        serde_json::from_value(valid_envelope()).expect("fixture envelope");
    BTreeMap::from([(UNIT.to_owned(), envelope.seal().unwrap())])
}

fn receipts(classes: &[&str]) -> Vec<Receipt> {
    classes
        .iter()
        .map(|class| {
            let mut value = valid_receipt();
            value["class"] = serde_json::json!(class);
            value["receiptId"] = serde_json::json!(format!("rc_{class}"));
            value["subject"] = serde_json::json!({
                "artifactEnvelopeDigest": digest(1),
                "unitIds": [UNIT]
            });
            serde_json::from_value::<Receipt>(value)
                .expect("fixture receipt")
                .seal()
                .unwrap()
        })
        .collect()
}

fn freshness() -> FreshnessPolicy {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../release/policy/freshness.toml"),
    )
    .expect("the shipped freshness policy");
    toml::from_str(&text).expect("the shipped freshness policy must parse")
}

fn required() -> BTreeMap<String, Vec<String>> {
    BTreeMap::from([(
        "rust-lambda".to_owned(),
        vec![
            "unit".to_owned(),
            "lint".to_owned(),
            "sbom".to_owned(),
            "license".to_owned(),
            "vulnerability".to_owned(),
        ],
    )])
}

fn now() -> time::OffsetDateTime {
    time::OffsetDateTime::parse(
        "2026-08-01T02:00:00Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap()
}

fn statement(manifest: &CompositionManifest) -> VerificationStatement {
    let mut value = valid_statement();
    value["releaseId"] = serde_json::json!(manifest.release_id);
    serde_json::from_value::<VerificationStatement>(value)
        .expect("fixture statement")
        .seal()
        .unwrap()
}

struct Case {
    manifest: CompositionManifest,
    envelopes: BTreeMap<String, ArtifactEnvelope>,
    receipts: Vec<Receipt>,
    statement: Option<VerificationStatement>,
    plane: Plane,
    readiness: OperationalReadiness,
    applied_head: Option<String>,
    applied_generation: Option<u32>,
}

impl Case {
    fn dev() -> Self {
        let manifest = manifest();
        Self {
            envelopes: envelopes(),
            receipts: receipts(&["unit", "lint", "sbom", "license", "vulnerability"]),
            statement: None,
            plane: Plane::Dev,
            readiness: OperationalReadiness {
                backup_ready: true,
                quota_headroom: true,
                within_maintenance_window: true,
                approval_valid: true,
            },
            applied_head: Some("20260801000100".to_owned()),
            applied_generation: Some(1),
            manifest,
        }
    }

    fn prd() -> Self {
        let mut case = Self::dev();
        case.statement = Some(statement(&case.manifest));
        case.plane = Plane::Prd;
        case
    }

    fn run(&self) -> aex_release_tool::error::Result<aex_release_tool::admit::Admission> {
        let required = required();
        let freshness = freshness();
        let builders = vec![BUILDER.to_owned()];
        admit(&AdmissionInputs {
            manifest: &self.manifest,
            envelopes: &self.envelopes,
            receipts: &self.receipts,
            statement: self.statement.as_ref(),
            freshness: &freshness,
            plane: self.plane,
            readiness: self.readiness,
            required_receipts: &required,
            builder_allowlist: &builders,
            applied_central_head: self.applied_head.clone(),
            applied_regional_generation: self.applied_generation,
            now: now(),
        })
    }
}

#[test]
fn a_complete_candidate_is_admitted_to_dev_and_to_production() {
    Case::dev().run().expect("dev admission");
    Case::prd().run().expect("production admission");
}

#[test]
fn rule_one_a_digest_that_does_not_match_the_manifest_fails() {
    let mut case = Case::dev();
    case.envelopes.get_mut(UNIT).unwrap().output.digest = digest(0x99);
    let err = case.run().unwrap_err();
    assert_eq!(err.exit.code(), 21);
    assert!(err.rules().contains(&"admit-digest-mismatch"));
}

#[test]
fn rule_one_a_missing_envelope_fails() {
    let mut case = Case::dev();
    case.envelopes.clear();
    let err = case.run().unwrap_err();
    assert_eq!(err.exit.code(), 21);
    assert!(err.rules().contains(&"admit-envelope-missing"));
}

#[test]
fn rule_two_an_unallowlisted_builder_fails() {
    let mut case = Case::dev();
    case.envelopes.get_mut(UNIT).unwrap().provenance.builder_id =
        "https://example.invalid/builder".to_owned();
    let err = case.run().unwrap_err();
    assert_eq!(err.exit.code(), 22);
    assert!(err.rules().contains(&"admit-builder-not-allowlisted"));
}

#[test]
fn rule_two_a_build_from_a_branch_other_than_main_fails() {
    let mut case = Case::dev();
    case.envelopes.get_mut(UNIT).unwrap().source.r#ref = Some("refs/heads/feature".to_owned());
    let err = case.run().unwrap_err();
    assert_eq!(err.exit.code(), 22);
    assert!(err.rules().contains(&"admit-source-ref"));
}

#[test]
fn rule_three_a_toolchain_or_contract_drift_fails() {
    let mut case = Case::dev();
    case.envelopes
        .get_mut(UNIT)
        .unwrap()
        .inputs
        .toolchain
        .channel = "1.98.0".to_owned();
    let err = case.run().unwrap_err();
    assert_eq!(err.exit.code(), 30);
    assert!(err.rules().contains(&"admit-toolchain-drift"));

    let mut case = Case::dev();
    case.envelopes
        .get_mut(UNIT)
        .unwrap()
        .identities
        .contract_digest = digest(0x77);
    let err = case.run().unwrap_err();
    assert!(err.rules().contains(&"admit-contract-drift"));
}

#[test]
fn rule_five_a_missing_required_receipt_class_fails() {
    let mut case = Case::dev();
    case.receipts = receipts(&["unit", "lint"]);
    let err = case.run().unwrap_err();
    assert_eq!(err.exit.code(), 40);
    assert!(err.rules().contains(&"admit-receipt-missing"));
}

#[test]
fn rule_five_a_receipt_reporting_a_skip_fails() {
    let mut case = Case::dev();
    case.receipts[0].inventory.skipped = 1;
    let err = case.run().unwrap_err();
    assert_eq!(err.exit.code(), 41);
}

#[test]
fn rule_five_a_stale_receipt_fails() {
    let mut case = Case::prd();
    // `smoke` is release-bound and expires in 24 hours.
    let mut value = valid_receipt();
    value["class"] = serde_json::json!("smoke");
    value["receiptId"] = serde_json::json!("rc_smoke");
    value["layer"] = serde_json::json!("smoke");
    value["completedAt"] = serde_json::json!("2026-07-20T00:00:00Z");
    value["subject"] = serde_json::json!({
        "releaseId": case.manifest.release_id,
        "unitIds": [UNIT]
    });
    case.receipts.push(
        serde_json::from_value::<Receipt>(value)
            .unwrap()
            .seal()
            .unwrap(),
    );
    let err = case.run().unwrap_err();
    assert_eq!(err.exit.code(), 42);
    assert!(err.rules().contains(&"evidence-stale"));
}

#[test]
fn rule_six_an_unapproved_advisory_fails() {
    let mut case = Case::dev();
    case.envelopes
        .get_mut(UNIT)
        .unwrap()
        .vulnerabilities
        .unapproved_critical = 1;
    let err = case.run().unwrap_err();
    assert_eq!(err.exit.code(), 23);
    assert!(err.rules().contains(&"admit-advisory-denied"));
}

#[test]
fn rule_seven_production_without_a_dev_rehearsal_fails() {
    let mut case = Case::prd();
    case.statement = None;
    let err = case.run().unwrap_err();
    assert_eq!(err.exit.code(), 43);
    assert!(err.rules().contains(&"admit-verification-missing"));
}

#[test]
fn rule_seven_a_statement_for_another_release_fails() {
    let mut case = Case::prd();
    let mut value = valid_statement();
    value["releaseId"] = serde_json::json!(digest(0xEE));
    case.statement = Some(
        serde_json::from_value::<VerificationStatement>(value)
            .unwrap()
            .seal()
            .unwrap(),
    );
    let err = case.run().unwrap_err();
    assert_eq!(err.exit.code(), 43);
    assert!(err.rules().contains(&"statement-release-mismatch"));
}

#[test]
fn rule_seven_a_statement_that_skipped_a_unit_is_not_a_whole_manifest_verification() {
    let mut case = Case::prd();
    let mut value = valid_statement();
    value["releaseId"] = serde_json::json!(case.manifest.release_id);
    value["deployed"] = serde_json::json!([{
        "unit": "some-other-unit",
        "expectedDigest": digest(5),
        "actualDigest": digest(5),
        "actualVersionOrAlias": "live",
        "readbackAt": "2026-08-01T01:00:00Z"
    }]);
    case.statement = Some(
        serde_json::from_value::<VerificationStatement>(value)
            .unwrap()
            .seal()
            .unwrap(),
    );
    let err = case.run().unwrap_err();
    assert!(err.rules().contains(&"statement-incomplete"));
}

#[test]
fn rule_eight_a_required_schema_head_above_the_applied_head_fails() {
    let mut case = Case::dev();
    case.manifest
        .units
        .get_mut(UNIT)
        .unwrap()
        .required_central_head = Some("20260901000000".to_owned());
    case.manifest = case.manifest.clone().seal().unwrap();
    let err = case.run().unwrap_err();
    assert_eq!(err.exit.code(), 32);
    assert!(err.rules().contains(&"admit-head-mismatch"));
}

#[test]
fn rule_eight_a_regional_generation_mismatch_fails() {
    let mut case = Case::dev();
    case.applied_generation = Some(2);
    let err = case.run().unwrap_err();
    assert_eq!(err.exit.code(), 32);
    assert!(err.rules().contains(&"admit-regional-generation-mismatch"));
}

#[test]
fn rule_nine_an_unready_plane_denies_production_and_not_dev() {
    let mut case = Case::prd();
    case.readiness.backup_ready = false;
    let err = case.run().unwrap_err();
    assert_eq!(err.exit.code(), 44);
    assert!(err.rules().contains(&"admit-operational-precondition"));

    let mut case = Case::dev();
    case.readiness = OperationalReadiness::default();
    case.run()
        .expect("dev deployment does not wait on a restore drill");
}

#[test]
fn the_shipped_freshness_policy_declares_every_class_the_receipt_schema_permits() {
    let policy = freshness();
    let schema =
        aex_release_tool::schemas::document(aex_release_tool::schemas::SchemaName::EvidenceReceipt)
            .unwrap();
    let classes = schema["properties"]["class"]["enum"]
        .as_array()
        .expect("the receipt schema enumerates its classes");
    let missing: Vec<&str> = classes
        .iter()
        .filter_map(serde_json::Value::as_str)
        .filter(|class| !policy.class.contains_key(*class))
        .collect();
    assert!(
        missing.is_empty(),
        "these receipt classes have no freshness rule and would be permanently fresh: \
         {missing:?}"
    );
}
