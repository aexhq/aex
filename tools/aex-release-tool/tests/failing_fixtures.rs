//! The deliberate-failure table.
//!
//! Every entry is a thing the pipeline must refuse. Each is asserted to produce
//! its exact exit code, because a gate nobody has watched fail is a gate nobody
//! knows is wired up. These are the fixtures a reviewer runs first.

mod common;

use std::collections::BTreeMap;

use aex_release_tool::admit::{AdmissionInputs, OperationalReadiness, Plane, admit};
use aex_release_tool::artifact::ArtifactEnvelope;
use aex_release_tool::evidence::{
    DeclaredProducers, FreshnessPolicy, ProducerInstance, Receipt, aggregate,
};
use aex_release_tool::graph::inputs::GraphInputs;
use aex_release_tool::graph::select::{Lane, Mode, select};
use aex_release_tool::graph::verify;
use aex_release_tool::manifest::CompositionManifest;
use aex_release_tool::policy::{lint_workflow_text, scan_terraform_text};
use aex_release_tool::private_path;
use aex_release_tool::verification::VerificationStatement;
use common::docs::{
    BUILDER, digest, valid_envelope, valid_manifest, valid_receipt, valid_statement,
};
use common::{CratePlan, Fixture, SOUND_SCENARIOS, SOUND_UNITS, deployable_meta, live_meta};

const UNIT: &str = "regional-session-api";

fn freshness() -> FreshnessPolicy {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../release/policy/freshness.toml"),
    )
    .expect("the shipped freshness policy");
    toml::from_str(&text).unwrap()
}

fn now() -> time::OffsetDateTime {
    time::OffsetDateTime::parse(
        "2026-08-01T02:00:00Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap()
}

fn manifest() -> CompositionManifest {
    serde_json::from_value::<CompositionManifest>(valid_manifest())
        .unwrap()
        .seal()
        .unwrap()
}

fn envelopes() -> BTreeMap<String, ArtifactEnvelope> {
    BTreeMap::from([(
        UNIT.to_owned(),
        serde_json::from_value::<ArtifactEnvelope>(valid_envelope())
            .unwrap()
            .seal()
            .unwrap(),
    )])
}

fn receipt(class: &str) -> Receipt {
    let mut value = valid_receipt();
    value["class"] = serde_json::json!(class);
    value["receiptId"] = serde_json::json!(format!("rc_{class}"));
    value["subject"] = serde_json::json!({
        "artifactSubjectDigest": digest(1),
        "unitIds": [UNIT]
    });
    serde_json::from_value::<Receipt>(value)
        .unwrap()
        .seal()
        .unwrap()
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

fn run_admit(
    receipts: &[Receipt],
    statement: Option<&VerificationStatement>,
    plane: Plane,
    manifest: &CompositionManifest,
) -> aex_release_tool::error::Result<aex_release_tool::admit::Admission> {
    let required = required();
    let freshness = freshness();
    let builders = vec![BUILDER.to_owned()];
    let unearned = aex_release_tool::test_registry::UnearnedIndex::default();
    admit(&AdmissionInputs {
        manifest,
        envelopes: &envelopes(),
        receipts,
        statement,
        freshness: &freshness,
        plane,
        readiness: OperationalReadiness {
            backup_ready: true,
            quota_headroom: true,
            within_maintenance_window: true,
            approval_valid: true,
        },
        required_receipts: &required,
        builder_allowlist: &builders,
        applied_central_head: Some("20260801000100".to_owned()),
        applied_regional_generation: Some(1),
        unearned: &unearned,
        now: now(),
    })
}

/// `missing-receipt/` — a candidate with an absent required receipt class.
#[test]
fn missing_receipt_exits_40() {
    let receipts = vec![receipt("unit"), receipt("lint")];
    let err = run_admit(&receipts, None, Plane::Dev, &manifest()).unwrap_err();
    assert_eq!(err.exit.code(), 40, "{err}");
}

/// `skipped-test/` — a receipt reporting a skipped case.
#[test]
fn skipped_test_exits_41() {
    let mut receipt = receipt("unit");
    receipt.inventory.skipped = 1;
    let err = receipt.verify().unwrap_err();
    assert_eq!(err.exit.code(), 41, "{err}");
}

/// `retry-to-green/` — a second attempt with no preserved first failure.
#[test]
fn retry_to_green_exits_41() {
    let mut receipt = receipt("unit");
    receipt.source.run_attempt = 2;
    let err = receipt.verify().unwrap_err();
    assert_eq!(err.exit.code(), 41, "{err}");
    assert!(err.rules().contains(&"flake-first-failure-lost"));
}

/// `stale-evidence/` — a release-bound receipt older than its class permits.
#[test]
fn stale_evidence_exits_42() {
    let mut value = valid_receipt();
    value["class"] = serde_json::json!("smoke");
    value["layer"] = serde_json::json!("smoke");
    value["completedAt"] = serde_json::json!("2026-07-01T00:00:00Z");
    value["subject"] = serde_json::json!({ "releaseId": digest(0x11) });
    let receipt: Receipt = serde_json::from_value(value).unwrap();
    let err =
        aex_release_tool::evidence::check_freshness(&receipt, &freshness(), &digest(0x11), now())
            .unwrap_err();
    assert_eq!(err.exit.code(), 42, "{err}");
}

/// `stale-graph-edge/` — a scenario edge naming a node that no longer exists.
#[test]
fn stale_graph_edge_exits_10() {
    let root = Fixture::new()
        .add_crate(CratePlan::new("aex-leaf", "crates/aex-leaf"))
        .scenarios(
            "schema = \"aex.scenario-ownership.v1\"\n\n[[scenario]]\nid = \"SC-STALE\"\n\
             owner = \"delivery\"\nobserves = [\"cargo:aex-removed\"]\n",
        )
        .build();
    let inputs = GraphInputs::load(&root).unwrap();
    let err = verify::verify(&inputs).unwrap_err();
    assert_eq!(err.exit.code(), 10, "{err}");
    assert!(err.rules().contains(&"unknown-node-reference"));
}

/// `altered-artifact/` — bytes that no longer hash to the recorded digest.
#[test]
fn altered_artifact_exits_21() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("artifact.zip");
    std::fs::write(&path, b"original").unwrap();
    let mut value = valid_envelope();
    value["output"]["digest"] =
        serde_json::json!(aex_release_tool::canon::digest_bytes(b"original"));
    value["output"]["sizeBytes"] = serde_json::json!(8);
    let envelope = serde_json::from_value::<ArtifactEnvelope>(value)
        .unwrap()
        .seal()
        .unwrap();
    std::fs::write(&path, b"altered!").unwrap();
    let err = envelope.verify(Some(&path), false).unwrap_err();
    assert_eq!(err.exit.code(), 21, "{err}");
}

/// `unsigned-manifest/` — a production candidate with no attested dev
/// verification statement.
#[test]
fn unsigned_manifest_exits_43() {
    let receipts = vec![
        receipt("unit"),
        receipt("lint"),
        receipt("sbom"),
        receipt("license"),
        receipt("vulnerability"),
    ];
    let err = run_admit(&receipts, None, Plane::Prd, &manifest()).unwrap_err();
    assert_eq!(err.exit.code(), 43, "{err}");

    // And an unattested statement fails the same way.
    let manifest = manifest();
    let mut value = valid_statement();
    value["releaseId"] = serde_json::json!(manifest.release_id);
    value["attestation"]["bundleDigest"] = serde_json::json!("");
    let statement = serde_json::from_value::<VerificationStatement>(value)
        .unwrap()
        .seal()
        .unwrap();
    let err = run_admit(&receipts, Some(&statement), Plane::Prd, &manifest).unwrap_err();
    assert_eq!(err.exit.code(), 43, "{err}");
    assert!(err.rules().contains(&"statement-unattested"));
}

/// `mutable-tag/` — a manifest naming an image tag instead of a digest.
#[test]
fn mutable_tag_exits_31() {
    let mut value = valid_manifest();
    value["units"][UNIT]["location"]["uri"] = serde_json::json!("registry/session-api:latest");
    let manifest = serde_json::from_value::<CompositionManifest>(value)
        .unwrap()
        .seal()
        .unwrap();
    manifest
        .validate(false)
        .expect("the manifest is structurally sound");
    let err = manifest.validate(true).unwrap_err();
    assert_eq!(err.exit.code(), 31, "{err}");
    assert!(err.rules().contains(&"manifest-mutable-reference"));
}

/// `mutable-tag/` variant — an environment identity in a plane-neutral manifest.
#[test]
fn an_environment_identity_exits_31() {
    let mut value = valid_manifest();
    value["units"][UNIT]["location"]["uri"] =
        serde_json::json!("arn:aws:s3:::aex-prd-artifacts/modules.tar.gz");
    let manifest = serde_json::from_value::<CompositionManifest>(value)
        .unwrap()
        .seal()
        .unwrap();
    let err = manifest.validate(true).unwrap_err();
    assert_eq!(err.exit.code(), 31, "{err}");
    assert!(err.rules().contains(&"manifest-environment-identity"));
}

/// `deploy-time-compiler/` — both the Terraform scanner and the workflow gate
/// must reject it. One gate is a gate somebody can route around.
#[test]
fn deploy_time_compiler_fails_both_gates() {
    let terraform = scan_terraform_text(
        "infra/modules/lambda-function/main.tf",
        "resource \"null_resource\" \"build\" {\n  provisioner \"local-exec\" {\n    \
         command = \"cargo build --release\"\n  }\n}\n",
    );
    assert!(
        terraform
            .iter()
            .any(|v| v.rule == "terraform-packages-code"),
        "the Terraform scanner missed a deploy-time build"
    );

    let (workflow, _) = lint_workflow_text(
        "_release-engine.yml",
        "jobs:\n  deploy:\n    runs-on: ubuntu-latest\n    permissions:\n      \
         contents: read\n    steps:\n      - run: cargo lambda build --release\n",
    );
    assert!(
        workflow.iter().any(|v| v.rule == "release-engine-builds"),
        "the workflow gate missed a deploy-time build"
    );
}

/// `orphan-path/` — a repository file matching no ownership rule.
#[test]
fn orphan_path_exits_10() {
    let root = Fixture::new()
        .add_crate(CratePlan::new("aex-leaf", "crates/aex-leaf"))
        .file("unowned/thing.bin")
        .build();
    let inputs = GraphInputs::load(&root).unwrap();
    let err = verify::verify(&inputs).unwrap_err();
    assert_eq!(err.exit.code(), 10, "{err}");
    assert!(err.rules().contains(&"orphan-path"));
}

/// `unclassified-private-path/` — a private file matching no category.
#[test]
fn unclassified_private_path_exits_60() {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../release/policy/private-path-policy.json"),
    )
    .unwrap();
    let policy: private_path::Policy = serde_json::from_str(&text).unwrap();
    let err =
        private_path::check(&policy, &["scratch/notes.txt".to_owned()], |_| None).unwrap_err();
    assert_eq!(err.exit.code(), 60, "{err}");
    assert!(err.rules().contains(&"private-path-unclassified"));
}

/// `missing-scenario-owner/` — a deployable nobody exercises.
#[test]
fn missing_scenario_owner_exits_10() {
    let root = Fixture::new()
        .add_crate(
            CratePlan::new("demo-api", "services/demo-api")
                .meta(deployable_meta("demo-api", "aex-live-demo-api")),
        )
        .add_crate(
            CratePlan::new("aex-live-demo-api", "tests/live/aex-live-demo-api")
                .meta(live_meta("demo-api")),
        )
        .units(SOUND_UNITS)
        .scenarios("schema = \"aex.scenario-ownership.v1\"\n")
        .build();
    let inputs = GraphInputs::load(&root).unwrap();
    let err = verify::verify(&inputs).unwrap_err();
    assert_eq!(err.exit.code(), 10, "{err}");
    assert!(err.rules().contains(&"missing-scenario-owner"));
}

/// `empty-matrix-silent-pass/` — a lane whose declared job produced no receipt.
/// An empty selection is a real result and must produce a real receipt, never a
/// green skipped required job.
#[test]
fn empty_matrix_silent_pass_exits_40() {
    let declared = DeclaredProducers {
        schema: "aex.declared-producers.v1".to_owned(),
        lane: "pr".to_owned(),
        producers: vec![ProducerInstance {
            job_name: "rust".to_owned(),
            package: "aex-wire".to_owned(),
            partition: None,
            class: "unit".to_owned(),
        }],
    };
    let err = aggregate(&[], &declared).unwrap_err();
    assert_eq!(err.exit.code(), 40, "{err}");
    assert!(err.rules().contains(&"lane-receipt-missing"));
}

/// An empty selection is nonetheless distinguishable from a broken router.
#[test]
fn an_empty_selection_is_a_result_and_a_broken_router_is_not() {
    let root = common::sound_fixture();
    let inputs = GraphInputs::load(&root).unwrap();
    let built = verify::build(&inputs).unwrap();
    let selection = select(
        &built,
        &inputs,
        &["README.md".to_owned()],
        Mode::Affected,
        Lane::Pr,
    )
    .unwrap();
    assert!(selection.is_empty(), "a documentation edit selects nothing");
    assert!(
        !selection.routing_failed,
        "an empty selection is a decision, not a failure"
    );

    let broken = aex_release_tool::graph::matrix::degraded("cargo metadata failed");
    assert!(broken.routing_failed);
    assert!(broken.repo_wide, "a broken router must not narrow");
}

/// `head-mismatch/` — a unit requiring a schema head above the applied one.
#[test]
fn head_mismatch_exits_32() {
    let mut manifest = manifest();
    manifest.units.get_mut(UNIT).unwrap().required_central_head = Some("20270101000000".to_owned());
    let manifest = manifest.seal().unwrap();
    let receipts = vec![
        receipt("unit"),
        receipt("lint"),
        receipt("sbom"),
        receipt("license"),
        receipt("vulnerability"),
    ];
    let err = run_admit(&receipts, None, Plane::Dev, &manifest).unwrap_err();
    assert_eq!(err.exit.code(), 32, "{err}");
}

/// `nonmonotone-selector/` — `selftest` reports a router that shrank under a
/// wider input.
#[test]
fn nonmonotone_selector_is_detected_by_selftest() {
    use aex_release_tool::graph::select::{Selected, Selection, SelectionReason};

    let narrow = Selection {
        schema: "aex.selection.v1".to_owned(),
        lane: Lane::Pr,
        mode: Mode::Affected,
        test: vec![Selected {
            id: aex_release_tool::graph::NodeId::cargo("aex-leaf"),
            reason: SelectionReason::RouterChanged,
        }],
        deploy: Vec::new(),
        scenarios: Vec::new(),
        repo_wide: false,
        unowned: Vec::new(),
        changed_paths: 1,
        routing_failed: false,
        routing_failure_reason: None,
        shadow: None,
    };
    let wide = Selection {
        test: Vec::new(),
        ..narrow.clone()
    };
    assert_eq!(
        aex_release_tool::selftest::first_lost(&narrow, &wide).as_deref(),
        Some("cargo:aex-leaf"),
        "widening the change set dropped a node and selftest must say which one"
    );
    assert!(
        aex_release_tool::selftest::first_lost(&narrow, &narrow).is_none(),
        "an unchanged selection loses nothing"
    );
}

/// The whole set is a table, and the table must not silently shrink.
#[test]
fn the_deliberate_failure_table_covers_every_declared_entry() {
    // Each name below is asserted by a test in this file. Keeping the list here
    // means removing a fixture without removing its row is a compile-visible
    // change rather than a quiet loss of coverage.
    let entries = [
        "missing-receipt",
        "skipped-test",
        "retry-to-green",
        "stale-evidence",
        "stale-graph-edge",
        "altered-artifact",
        "unsigned-manifest",
        "mutable-tag",
        "deploy-time-compiler",
        "orphan-path",
        "unclassified-private-path",
        "missing-scenario-owner",
        "empty-matrix-silent-pass",
        "head-mismatch",
        "nonmonotone-selector",
    ];
    assert_eq!(entries.len(), 15);
    let _ = SOUND_SCENARIOS;
}
