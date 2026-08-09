//! A published envelope can be minted only from file-backed CI evidence.

mod common;

use std::collections::BTreeMap;

use aex_release_tool::artifact::{
    ArtifactEnvelope, Licenses, Location, Signature, Toolchain, Vulnerabilities, Workflow, plan,
};
use aex_release_tool::canon;
use aex_release_tool::certification::{ExpectedSource, defer, inventory};
use aex_release_tool::certify::{
    CertificationClaims, CertificationFiles, CertificationProvenance, certify,
};
use aex_release_tool::describe::{LocalBuild, UnearnedField, describe};
use aex_release_tool::evidence::{FreshnessPolicy, Receipt};
use aex_release_tool::graph::inputs::{Unit, Units};
use common::docs::{BUILDER, digest, sha1, valid_receipt};

/// The subject every fixture below certifies.
///
/// These tests are about certifying a blob: file-backed evidence, a
/// `github-release` location and an asset name derived from `form = "zip"`.
/// The unit must therefore be a shipped Lambda unit — an OCI unit has no blob
/// to publish, needs an inspected image identity before it can even be
/// described, and is refused a `github-release` location by
/// `github_release_unit_uri`. `regional-observation-api` is the shipped
/// request-path Lambda that supplies that shape and whose `required_receipts`
/// still carry both the `unit`/`lint` pair and the `contract` class the
/// deferral tests split on.
fn unit() -> Unit {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../release/units.toml"),
    )
    .unwrap();
    let units: Units = toml::from_str(&text).unwrap();
    units
        .units
        .into_iter()
        .find(|unit| unit.id == "regional-observation-api")
        .unwrap()
}

fn freshness() -> FreshnessPolicy {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../release/policy/freshness.toml"),
    )
    .unwrap();
    toml::from_str(&text).unwrap()
}

fn receipt(class: &str, unit: &str, artifact_subject_digest: &str) -> Receipt {
    let mut value = valid_receipt();
    value["receiptId"] = serde_json::json!(format!("rc_{class}"));
    value["class"] = serde_json::json!(class);
    value["lane"] = serde_json::json!("main");
    value["subject"] = serde_json::json!({
        "artifactSubjectDigest": artifact_subject_digest,
        "unitIds": [unit]
    });
    serde_json::from_value::<Receipt>(value)
        .unwrap()
        .seal()
        .unwrap()
}

struct Fixture {
    temp: tempfile::TempDir,
    unit: Unit,
    draft: aex_release_tool::artifact::ArtifactEnvelope,
    claims: CertificationClaims,
    artifact: std::path::PathBuf,
    sbom: std::path::PathBuf,
    licenses: std::path::PathBuf,
    vulnerabilities: std::path::PathBuf,
    provenance: std::path::PathBuf,
    receipts: Vec<Receipt>,
}

impl Fixture {
    #[allow(clippy::too_many_lines)]
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let unit = unit();
        let recipe = plan(&unit).unwrap();
        let artifact = temp.path().join("artifact.zip");
        std::fs::write(&artifact, b"PK\x03\x04certified bytes").unwrap();
        let build = LocalBuild {
            unit: &unit,
            plan: &recipe,
            artifact: &artifact,
            oci_identity: None,
            repository: "aexhq/aex".to_owned(),
            commit_sha: sha1(),
            tree_clean: true,
            git_ref: Some("refs/heads/main".to_owned()),
            toolchain: Toolchain {
                channel: "1.97.1".to_owned(),
                rustc_version: "1.97.1".to_owned(),
                rustc_commit_hash: sha1(),
                host: "x86_64-unknown-linux-gnu".to_owned(),
                target: unit.target.clone(),
                components: Vec::new(),
                packager_version: Some("cargo-lambda 1.8.6".to_owned()),
            },
            lockfile_digest: digest(2),
            contract_digest: digest(6),
            actual_argv: None,
            closure: BTreeMap::from([("Cargo.lock".to_owned(), digest(2))]),
            location_uri: artifact.display().to_string(),
            receipts: Vec::new(),
            created_at: "2026-08-01T00:00:00Z".to_owned(),
        };
        let (draft, _) = describe(&build).unwrap();
        let uri = aex_release_tool::publication::github_release_unit_uri(
            "aexhq/aex",
            &sha1(),
            "123",
            1,
            &unit.id,
            &draft.output.digest,
            &unit.form,
        )
        .unwrap();
        let workflow = Workflow {
            repository: "aexhq/aex".to_owned(),
            r#ref: "refs/heads/main".to_owned(),
            path: ".github/workflows/_build-artifacts.yml".to_owned(),
            run_id: "123".to_owned(),
            run_attempt: 1,
            job_name: "regional-observation-api".to_owned(),
            builder_id: BUILDER.to_owned(),
        };
        let mut claims = CertificationClaims {
            workflow,
            location: Location {
                kind: "github-release".to_owned(),
                uri,
                immutable: true,
                object_version_id: None,
            },
            sbom_format: "cyclonedx-1.6".to_owned(),
            sbom_uri: String::new(),
            licenses: Licenses {
                policy_digest: digest(8),
                verdict: "allowed".to_owned(),
                denials: Vec::new(),
                inventory_digest: None,
            },
            vulnerabilities: Vulnerabilities {
                scanner: "cargo-deny".to_owned(),
                database: "rustsec-commit-0123456789abcdef".to_owned(),
                scanned_at: "2026-08-01T00:00:00Z".to_owned(),
                unapproved_critical: 0,
                unapproved_high: 0,
                approved_exceptions: Vec::new(),
            },
            provenance: CertificationProvenance {
                predicate_type: "https://slsa.dev/provenance/v1".to_owned(),
                uri: Some("https://github.com/aexhq/aex/attestations/123".to_owned()),
                builder_id: BUILDER.to_owned(),
                attested: true,
            },
            signature: Signature {
                present: false,
                kind: "none".to_owned(),
                key_id: None,
                bundle_digest: None,
            },
        };
        let sbom = temp.path().join("unit.cdx.json");
        std::fs::write(
            &sbom,
            serde_json::to_vec(&serde_json::json!({
                "bomFormat": "CycloneDX",
                "specVersion": "1.6",
                "metadata": {
                    "properties": [{
                        "name": "aex:artifactSubjectDigest",
                        "value": draft.artifact_subject_digest,
                    }],
                },
                "components": [{"name": "aex-wire"}],
            }))
            .unwrap(),
        )
        .unwrap();
        let sbom_digest = aex_release_tool::canon::digest_bytes(&std::fs::read(&sbom).unwrap());
        claims.sbom_uri = aex_release_tool::publication::github_release_aux_uri(
            "aexhq/aex",
            &sha1(),
            "123",
            1,
            &unit.id,
            &sbom_digest,
            aex_release_tool::publication::AuxiliaryAsset {
                class: "sbom",
                extension: "cdx.json",
            },
        )
        .unwrap();
        let licenses = temp.path().join("licenses.json");
        std::fs::write(
            &licenses,
            serde_json::to_vec(&serde_json::json!({
                "schema": "aex.license-inventory.v1",
                "unit": unit.id.clone(),
                "artifactSubjectDigest": draft.artifact_subject_digest.clone(),
                "policyDigest": claims.licenses.policy_digest.clone(),
                "components": [{
                    "name": "aex-wire",
                    "version": "0.1.0",
                    "licenses": ["Apache-2.0"],
                    "denied": [],
                }],
            }))
            .unwrap(),
        )
        .unwrap();
        let vulnerabilities = temp.path().join("vulnerabilities.json");
        std::fs::write(
            &vulnerabilities,
            serde_json::to_vec(&serde_json::json!({
                "schema": "aex.vulnerability-verdict.v1",
                "unit": unit.id.clone(),
                "artifactSubjectDigest": draft.artifact_subject_digest.clone(),
                "scanner": claims.vulnerabilities.scanner.clone(),
                "database": claims.vulnerabilities.database.clone(),
                "scannedAt": claims.vulnerabilities.scanned_at.clone(),
                "unapprovedCritical": claims.vulnerabilities.unapproved_critical,
                "unapprovedHigh": claims.vulnerabilities.unapproved_high,
                "matches": 0,
            }))
            .unwrap(),
        )
        .unwrap();
        let provenance = temp.path().join("attestation.json");
        std::fs::write(&provenance, br#"{"verificationMaterial":{}}"#).unwrap();
        let receipts = unit
            .required_receipts
            .iter()
            .map(|class| receipt(class, &unit.id, &draft.artifact_subject_digest))
            .collect();
        Self {
            temp,
            unit,
            draft,
            claims,
            artifact,
            sbom,
            licenses,
            vulnerabilities,
            provenance,
            receipts,
        }
    }

    fn certify(
        &self,
    ) -> aex_release_tool::error::Result<aex_release_tool::artifact::ArtifactEnvelope> {
        certify(
            self.draft.clone(),
            &self.unit,
            self.claims.clone(),
            CertificationFiles {
                artifact: Some(&self.artifact),
                sbom: Some(&self.sbom),
                license_inventory: Some(&self.licenses),
                vulnerability_verdict: Some(&self.vulnerabilities),
                provenance_bundle: &self.provenance,
                signature_bundle: None,
                defer_supply_chain: false,
            },
            &self.receipts,
            &freshness(),
        )
    }
}

fn assert_envelope_rule(
    envelope: ArtifactEnvelope,
    artifact: &std::path::Path,
    expected_rule: &str,
) {
    let error = envelope
        .seal()
        .unwrap()
        .verify(Some(artifact), false)
        .unwrap_err();
    assert!(
        error.rules().contains(&expected_rule),
        "expected `{expected_rule}`, got {:?}",
        error.rules()
    );
}

#[test]
fn certification_claims_omit_the_file_backed_provenance_digest() {
    let fixture = Fixture::new();
    let value = serde_json::to_value(&fixture.claims).unwrap();
    assert!(value["provenance"].get("bundleDigest").is_none());
    serde_json::from_value::<CertificationClaims>(value).unwrap();
}

#[test]
fn startup_certification_defers_scanners_without_deferring_publication() {
    let fixture = Fixture::new();
    let mut claims = serde_json::to_value(&fixture.claims).unwrap();
    let object = claims.as_object_mut().unwrap();
    for field in ["sbomFormat", "sbomUri", "licenses", "vulnerabilities"] {
        object.remove(field);
    }
    let claims = serde_json::from_value::<CertificationClaims>(claims).unwrap();
    let envelope = certify(
        fixture.draft.clone(),
        &fixture.unit,
        claims.clone(),
        CertificationFiles {
            artifact: Some(&fixture.artifact),
            sbom: None,
            license_inventory: None,
            vulnerability_verdict: None,
            provenance_bundle: &fixture.provenance,
            signature_bundle: None,
            defer_supply_chain: true,
        },
        &fixture.receipts,
        &freshness(),
    )
    .unwrap();

    assert!(envelope.supply_chain_deferred);
    assert_eq!(envelope.sbom.format, "deferred-startup");
    assert_eq!(envelope.licenses.verdict, "deferred-startup");
    assert_eq!(envelope.vulnerabilities.scanner, "deferred-startup");
    envelope.verify(Some(&fixture.artifact), false).unwrap();
    let mut mixed = envelope.clone();
    mixed.sbom.format = "cyclonedx-1.6".to_owned();
    assert_envelope_rule(
        mixed,
        &fixture.artifact,
        "envelope-deferred-supply-chain-shape",
    );

    let mut strict_with_flag = fixture.certify().unwrap();
    strict_with_flag.supply_chain_deferred = true;
    assert_envelope_rule(
        strict_with_flag,
        &fixture.artifact,
        "envelope-deferred-supply-chain-shape",
    );

    let mut missing_flag = envelope.clone();
    missing_flag.supply_chain_deferred = false;
    assert_envelope_rule(
        missing_flag,
        &fixture.artifact,
        "envelope-deferred-supply-chain-flag",
    );

    let mut absent_flag = serde_json::to_value(&envelope).unwrap();
    absent_flag
        .as_object_mut()
        .unwrap()
        .remove("supplyChainDeferred");
    let absent_flag = serde_json::from_value::<ArtifactEnvelope>(absent_flag).unwrap();
    assert_envelope_rule(
        absent_flag,
        &fixture.artifact,
        "envelope-deferred-supply-chain-flag",
    );

    for class in ["deny", "sbom", "license", "vulnerability"] {
        let mut with_scanner_receipt = envelope.clone();
        let mut scanner_receipt = with_scanner_receipt.receipts[0].clone();
        scanner_receipt.class = class.to_owned();
        with_scanner_receipt.receipts.push(scanner_receipt);
        assert_envelope_rule(
            with_scanner_receipt,
            &fixture.artifact,
            "envelope-deferred-supply-chain-receipt",
        );
    }

    let receipts = fixture
        .receipts
        .iter()
        .filter(|receipt| receipt.class != "unit")
        .cloned()
        .collect::<Vec<_>>();
    let error = certify(
        fixture.draft,
        &fixture.unit,
        claims,
        CertificationFiles {
            artifact: Some(&fixture.artifact),
            sbom: None,
            license_inventory: None,
            vulnerability_verdict: None,
            provenance_bundle: &fixture.provenance,
            signature_bundle: None,
            defer_supply_chain: true,
        },
        &receipts,
        &freshness(),
    )
    .unwrap_err();
    assert!(error.rules().contains(&"certify-receipt-missing"));
}

#[test]
fn certification_derives_file_identities_and_required_receipt_refs() {
    let fixture = Fixture::new();
    let envelope = fixture.certify().unwrap();
    assert_eq!(envelope.output.location.kind, "github-release");
    assert_eq!(envelope.sbom.component_count, 1);
    assert_eq!(
        envelope.sbom.digest,
        aex_release_tool::canon::digest_bytes(&std::fs::read(&fixture.sbom).unwrap())
    );
    assert_eq!(
        envelope.licenses.inventory_digest.as_deref(),
        Some(
            aex_release_tool::canon::digest_bytes(&std::fs::read(&fixture.licenses).unwrap())
                .as_str()
        )
    );
    assert_eq!(
        envelope.provenance.bundle_digest,
        aex_release_tool::canon::digest_bytes(&std::fs::read(&fixture.provenance).unwrap())
    );
    assert_eq!(
        envelope.receipts.len(),
        fixture.unit.required_receipts.len()
    );
    assert_eq!(
        envelope.artifact_subject_digest,
        fixture.draft.artifact_subject_digest
    );
    assert_ne!(envelope.envelope_digest, fixture.draft.envelope_digest);
}

#[test]
fn certification_counts_the_cyclonedx_metadata_subject_as_a_component() {
    let mut fixture = Fixture::new();
    let mut sbom: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&fixture.sbom).unwrap()).unwrap();
    sbom.as_object_mut().unwrap().remove("components");
    sbom["metadata"]["component"] = serde_json::json!({
        "bom-ref": "subject",
        "type": "file",
        "name": "artifact.bin",
        "version": digest(0x61),
    });
    std::fs::write(&fixture.sbom, serde_json::to_vec(&sbom).unwrap()).unwrap();
    let sbom_digest = aex_release_tool::canon::digest_bytes(&std::fs::read(&fixture.sbom).unwrap());
    fixture.claims.sbom_uri = aex_release_tool::publication::github_release_aux_uri(
        "aexhq/aex",
        &sha1(),
        "123",
        1,
        &fixture.unit.id,
        &sbom_digest,
        aex_release_tool::publication::AuxiliaryAsset {
            class: "sbom",
            extension: "cdx.json",
        },
    )
    .unwrap();

    let envelope = fixture.certify().unwrap();
    assert_eq!(envelope.sbom.component_count, 1);
}

#[test]
fn certification_refuses_a_receipt_for_another_artifact_subject() {
    let mut fixture = Fixture::new();
    fixture.receipts[0].subject.artifact_subject_digest = Some(digest(0x7b));
    fixture.receipts[0] = fixture.receipts[0].clone().seal().unwrap();

    let err = fixture.certify().unwrap_err();
    assert!(err.rules().contains(&"certify-receipt-artifact-subject"));
}

#[test]
fn certification_recomputes_and_refuses_a_tampered_draft_subject() {
    let mut fixture = Fixture::new();
    fixture.draft.artifact_subject_digest = digest(0x7c);

    let err = fixture.certify().unwrap_err();
    assert!(err.rules().contains(&"certify-artifact-subject-mismatch"));
}

#[test]
fn cli_binds_preexisting_receipts_before_certification_inserts_their_refs() {
    let mut fixture = Fixture::new();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let draft_path = fixture.temp.path().join("draft-envelope.json");
    std::fs::write(&draft_path, canon::to_file_bytes(&fixture.draft).unwrap()).unwrap();

    let mut bound_receipts = Vec::new();
    for (index, receipt) in fixture.receipts.iter().enumerate() {
        let mut unbound = receipt.clone();
        unbound.subject.artifact_subject_digest = None;
        let unbound = unbound.seal().unwrap();
        let original_digest = unbound.receipt_digest.clone();
        let receipt_path = fixture.temp.path().join(format!("receipt-{index}.json"));
        let bound_path = fixture.temp.path().join(format!("bound-{index}.json"));
        std::fs::write(&receipt_path, canon::to_file_bytes(&unbound).unwrap()).unwrap();

        let output = std::process::Command::new(env!("CARGO_BIN_EXE_aex-release-tool"))
            .arg("--root")
            .arg(&root)
            .arg("evidence")
            .arg("bind-artifact")
            .arg("--receipt")
            .arg(&receipt_path)
            .arg("--envelope")
            .arg(&draft_path)
            .arg("--out")
            .arg(&bound_path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "bind-artifact failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let bound: Receipt = serde_json::from_slice(&std::fs::read(bound_path).unwrap()).unwrap();
        assert_ne!(bound.receipt_digest, original_digest);
        assert_eq!(
            bound.subject.artifact_subject_digest.as_deref(),
            Some(fixture.draft.artifact_subject_digest.as_str())
        );
        bound.verify().unwrap();
        bound_receipts.push(bound);
    }

    fixture.receipts = bound_receipts;
    let certified = fixture.certify().unwrap();
    assert_eq!(
        certified.artifact_subject_digest,
        fixture.draft.artifact_subject_digest
    );
}

#[test]
fn artifact_binding_refuses_tampered_scope_and_rebinding() {
    let fixture = Fixture::new();

    let mut wrong_scope = fixture.receipts[0].clone();
    wrong_scope.source.commit_sha = "b".repeat(40);
    wrong_scope.subject.artifact_subject_digest = None;
    let wrong_scope = wrong_scope.seal().unwrap();
    let err = aex_release_tool::evidence::bind_artifact(wrong_scope, &fixture.draft).unwrap_err();
    assert!(err.rules().contains(&"bind-artifact-scope"));

    let mut tampered_draft = fixture.draft.clone();
    tampered_draft.artifact_subject_digest = digest(0x7d);
    let err =
        aex_release_tool::evidence::bind_artifact(fixture.receipts[0].clone(), &tampered_draft)
            .unwrap_err();
    assert!(err.rules().contains(&"bind-artifact-subject-mismatch"));

    let mut rebound = fixture.receipts[0].clone();
    rebound.subject.artifact_subject_digest = Some(digest(0x7e));
    let rebound = rebound.seal().unwrap();
    let err = aex_release_tool::evidence::bind_artifact(rebound, &fixture.draft).unwrap_err();
    assert!(err.rules().contains(&"bind-artifact-rebind"));
}

#[test]
fn certification_refuses_tampered_bytes_missing_receipts_and_cross_run_evidence() {
    let mut fixture = Fixture::new();
    std::fs::write(&fixture.artifact, b"tampered").unwrap();
    let err = fixture.certify().unwrap_err();
    assert!(err.rules().contains(&"published-blob-digest-mismatch"));

    fixture = Fixture::new();
    fixture.receipts.pop();
    let err = fixture.certify().unwrap_err();
    assert!(err.rules().contains(&"certify-receipt-missing"));

    fixture = Fixture::new();
    fixture.receipts[0].source.workflow_run_id = "999".to_owned();
    fixture.receipts[0] = fixture.receipts[0].clone().seal().unwrap();
    let err = fixture.certify().unwrap_err();
    assert!(err.rules().contains(&"certify-receipt-binding"));
}

#[test]
fn certification_refuses_an_empty_sbom_and_mutable_or_wrong_host_location() {
    let mut fixture = Fixture::new();
    std::fs::write(&fixture.sbom, br#"{"components":[]}"#).unwrap();
    let err = fixture.certify().unwrap_err();
    assert!(err.rules().contains(&"certify-sbom-empty"));

    fixture = Fixture::new();
    fixture.claims.location.uri = fixture
        .claims
        .location
        .uri
        .replace("github.com", "example.com");
    let err = fixture.certify().unwrap_err();
    assert!(err.rules().contains(&"envelope-github-release-location"));
}

#[test]
fn certification_refuses_supply_documents_for_another_subject_or_scan() {
    let fixture = Fixture::new();
    let mut license: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&fixture.licenses).unwrap()).unwrap();
    license["artifactSubjectDigest"] = serde_json::json!(digest(0x71));
    std::fs::write(&fixture.licenses, serde_json::to_vec(&license).unwrap()).unwrap();
    let err = fixture.certify().unwrap_err();
    assert!(err.rules().contains(&"certify-license-inventory-binding"));

    let fixture = Fixture::new();
    let mut verdict: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&fixture.vulnerabilities).unwrap()).unwrap();
    verdict["database"] = serde_json::json!("unidentified-database");
    std::fs::write(
        &fixture.vulnerabilities,
        serde_json::to_vec(&verdict).unwrap(),
    )
    .unwrap();
    let err = fixture.certify().unwrap_err();
    assert!(
        err.rules()
            .contains(&"certify-vulnerability-verdict-binding")
    );
}

#[test]
fn a_deferral_binds_exact_available_receipts_and_names_every_missing_class() {
    let fixture = Fixture::new();
    let available: Vec<Receipt> = fixture
        .receipts
        .iter()
        .filter(|receipt| matches!(receipt.class.as_str(), "unit" | "lint"))
        .cloned()
        .collect();
    let unearned = vec![UnearnedField {
        pointer: "/sbom".to_owned(),
        reason: "no SBOM producer ran".to_owned(),
    }];
    let deferral = defer(
        &fixture.draft,
        &fixture.unit,
        fixture.claims.workflow.clone(),
        unearned,
        &available,
        &freshness(),
    )
    .unwrap();

    deferral.verify().unwrap();
    assert_eq!(
        deferral
            .available_receipts
            .iter()
            .map(|receipt| receipt.class.as_str())
            .collect::<Vec<_>>(),
        vec!["lint", "unit"]
    );
    assert!(!deferral.missing_receipts.contains(&"sbom".to_owned()));
    assert!(deferral.missing_receipts.contains(&"contract".to_owned()));
    assert!(!deferral.missing_receipts.contains(&"unit".to_owned()));
}

#[test]
fn a_deferral_cannot_hide_a_certifiable_artifact_or_survive_tampering() {
    let mut fixture = Fixture::new();
    fixture.unit.required_receipts = fixture
        .receipts
        .iter()
        .map(|receipt| receipt.class.clone())
        .collect();
    let err = defer(
        &fixture.draft,
        &fixture.unit,
        fixture.claims.workflow.clone(),
        Vec::new(),
        &fixture.receipts,
        &freshness(),
    )
    .unwrap_err();
    assert!(err.rules().contains(&"certification-deferral-no-blocker"));

    let mut deferral = defer(
        &fixture.draft,
        &fixture.unit,
        fixture.claims.workflow.clone(),
        vec![UnearnedField {
            pointer: "/provenance".to_owned(),
            reason: "attestation has not been issued".to_owned(),
        }],
        &fixture.receipts,
        &freshness(),
    )
    .unwrap();
    deferral.artifact_digest = digest(0x44);
    let err = deferral.verify().unwrap_err();
    assert!(
        err.rules()
            .contains(&"certification-deferral-digest-mismatch")
    );
}

#[test]
fn inventory_accounts_for_deferrals_but_never_promotes_them() {
    let fixture = Fixture::new();
    let commit_sha = sha1();
    let deferral = defer(
        &fixture.draft,
        &fixture.unit,
        fixture.claims.workflow.clone(),
        vec![UnearnedField {
            pointer: "/sbom".to_owned(),
            reason: "no SBOM producer ran".to_owned(),
        }],
        &fixture.receipts[..1],
        &freshness(),
    )
    .unwrap();
    let registry = Units {
        schema: "aex.units.v1".to_owned(),
        units: vec![fixture.unit.clone()],
    };
    let report = inventory(
        &registry,
        &[],
        &[deferral],
        &ExpectedSource {
            repository: "aexhq/aex",
            commit_sha: &commit_sha,
            run_id: "123",
            run_attempt: 1,
        },
    )
    .unwrap();
    let err = report.blocking_error().unwrap();
    assert_eq!(err.exit, aex_release_tool::error::Exit::EvidenceMissing);
    assert!(err.rules().contains(&"artifact-certification-deferred"));

    let mut incomplete = defer(
        &fixture.draft,
        &fixture.unit,
        fixture.claims.workflow.clone(),
        Vec::new(),
        &fixture.receipts[..1],
        &freshness(),
    )
    .unwrap();
    incomplete.missing_receipts.pop();
    let incomplete = incomplete.seal().unwrap();
    let err = inventory(
        &registry,
        &[],
        &[incomplete],
        &ExpectedSource {
            repository: "aexhq/aex",
            commit_sha: &commit_sha,
            run_id: "123",
            run_attempt: 1,
        },
    )
    .unwrap_err();
    assert!(
        err.rules()
            .contains(&"certification-deferral-missing-receipts")
    );
}

#[test]
fn inventory_rejects_draft_envelopes_and_disposition_holes() {
    let fixture = Fixture::new();
    let commit_sha = sha1();
    let registry = Units {
        schema: "aex.units.v1".to_owned(),
        units: vec![fixture.unit.clone()],
    };
    let expected = ExpectedSource {
        repository: "aexhq/aex",
        commit_sha: &commit_sha,
        run_id: "123",
        run_attempt: 1,
    };
    let err = inventory(&registry, &[fixture.draft], &[], &expected).unwrap_err();
    assert!(err.rules().contains(&"envelope-mutable-location"));

    let err = inventory(&registry, &[], &[], &expected).unwrap_err();
    assert!(err.rules().contains(&"certification-inventory-hole"));
}
