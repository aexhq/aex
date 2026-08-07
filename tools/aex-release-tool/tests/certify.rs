//! A published envelope can be minted only from file-backed CI evidence.

mod common;

use std::collections::BTreeMap;

use aex_release_tool::artifact::{
    Licenses, Location, Provenance, Signature, Toolchain, Vulnerabilities, Workflow, plan,
};
use aex_release_tool::certify::{CertificationClaims, CertificationFiles, certify};
use aex_release_tool::describe::{LocalBuild, describe};
use aex_release_tool::evidence::Receipt;
use aex_release_tool::graph::inputs::{Unit, Units};
use common::docs::{BUILDER, digest, sha1, valid_receipt};

fn unit() -> Unit {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../release/units.toml"),
    )
    .unwrap();
    let units: Units = toml::from_str(&text).unwrap();
    units
        .units
        .into_iter()
        .find(|unit| unit.id == "regional-session-api")
        .unwrap()
}

fn receipt(class: &str, unit: &str) -> Receipt {
    let mut value = valid_receipt();
    value["receiptId"] = serde_json::json!(format!("rc_{class}"));
    value["class"] = serde_json::json!(class);
    value["lane"] = serde_json::json!("main");
    value["subject"] = serde_json::json!({ "unitIds": [unit] });
    serde_json::from_value::<Receipt>(value)
        .unwrap()
        .seal()
        .unwrap()
}

struct Fixture {
    _temp: tempfile::TempDir,
    unit: Unit,
    draft: aex_release_tool::artifact::ArtifactEnvelope,
    claims: CertificationClaims,
    artifact: std::path::PathBuf,
    sbom: std::path::PathBuf,
    licenses: std::path::PathBuf,
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
            job_name: "regional-session-api".to_owned(),
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
            provenance: Provenance {
                predicate_type: "https://slsa.dev/provenance/v1".to_owned(),
                bundle_digest: String::new(),
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
            br#"{"bomFormat":"CycloneDX","specVersion":"1.6","components":[{"name":"aex-wire"}]}"#,
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
        std::fs::write(&licenses, br#"{"allowed":["Apache-2.0"]}"#).unwrap();
        let provenance = temp.path().join("attestation.json");
        std::fs::write(&provenance, br#"{"verificationMaterial":{}}"#).unwrap();
        let receipts = unit
            .required_receipts
            .iter()
            .map(|class| receipt(class, &unit.id))
            .collect();
        Self {
            _temp: temp,
            unit,
            draft,
            claims,
            artifact,
            sbom,
            licenses,
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
                sbom: &self.sbom,
                license_inventory: &self.licenses,
                provenance_bundle: &self.provenance,
            },
            &self.receipts,
        )
    }
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
        envelope.receipts.len(),
        fixture.unit.required_receipts.len()
    );
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
