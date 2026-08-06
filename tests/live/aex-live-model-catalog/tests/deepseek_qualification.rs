//! Black-box contract for the protected `DeepSeek` genesis qualification target.

use std::collections::BTreeSet;
use std::process::Command;

use aex_live_model_catalog::deepseek_qualification::{
    DEEPSEEK_FLASH_MODEL, NegativeInputs, ProtectedCredential, fault_evidence, programs, target,
    unsupported_required_probes,
};
use aex_live_model_catalog::executor::ProbeProgram;
use aex_model_catalog::document::Capability;
use aex_model_catalog::receipt::ProbeId;
use aex_wire::provider::ProviderId;

#[test]
fn the_genesis_target_is_exact_and_claims_only_the_streaming_text_baseline() {
    let target = target().expect("the reviewed target is valid");
    assert_eq!(target.provider, ProviderId::Deepseek);
    assert_eq!(target.model.as_str(), DEEPSEEK_FLASH_MODEL);
    assert_eq!(
        target.capabilities.declared(),
        vec![
            Capability::TextIn,
            Capability::TextOut,
            Capability::Streaming
        ]
    );
}

#[test]
fn every_probe_has_one_explicit_program_even_when_the_capability_is_not_claimed() {
    let programs = programs().expect("the reviewed program inventory is valid");
    assert_eq!(programs.len(), ProbeId::ALL.len());
    assert_eq!(
        programs
            .iter()
            .map(ProbeProgram::probe)
            .collect::<BTreeSet<_>>(),
        ProbeId::ALL.into_iter().collect()
    );
    assert!(
        programs
            .iter()
            .all(|program| program.target() == &target().unwrap())
    );
}

#[test]
fn protected_credentials_and_negative_inputs_never_render_secret_material() {
    let credential = ProtectedCredential::new("ds-secret-never-render".to_owned())
        .expect("a non-empty credential is admitted");
    let negative = NegativeInputs::for_run("30999999999", 2)
        .expect("run identity produces bounded negative inputs");
    let rendered = format!("{credential:?} {negative:?}");
    assert!(!rendered.contains("ds-secret-never-render"));
    assert!(!rendered.contains("30999999999"));
    assert!(negative.unknown_model().starts_with("aex-never-"));
    assert_ne!(negative.unknown_model(), DEEPSEEK_FLASH_MODEL);
}

#[test]
fn local_canned_faults_cannot_be_mistaken_for_live_provider_evidence() {
    for probe in ProbeId::ALL {
        assert!(fault_evidence(probe).is_none());
    }
}

#[test]
fn every_required_positive_program_has_a_production_path() {
    assert!(unsupported_required_probes().is_empty());
}

#[test]
fn preflight_is_machine_readable_and_ready() {
    let output = Command::new(env!("CARGO_BIN_EXE_aex-model-catalog-qualifier"))
        .arg("preflight")
        .output()
        .expect("the qualifier starts");
    assert!(output.status.success());
    let projection: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("preflight is JSON");
    assert_eq!(projection["provider"], "deepseek");
    assert_eq!(projection["model"], DEEPSEEK_FLASH_MODEL);
    assert_eq!(projection["programs"], 23);
    assert_eq!(projection["ready"], true);
    assert_eq!(
        projection["unsupportedRequired"].as_array().unwrap().len(),
        0
    );
}

#[test]
fn run_requires_the_pinned_tokenizer_path_before_reading_a_provider_credential() {
    let output = Command::new(env!("CARGO_BIN_EXE_aex-model-catalog-qualifier"))
        .args([
            "run",
            "--source-sha",
            "1234567890abcdef1234567890abcdef12345678",
            "--run-id",
            "30999999999",
            "--run-attempt",
            "2",
            "--maximum-budget-micro-usd",
            "25000",
            "--maximum-runtime-seconds",
            "1800",
            "--output-dir",
            ".tmp/model-catalog/qualification",
        ])
        .env_remove("AEX_LIVE_PROVIDER_KEY_DEEPSEEK")
        .env_remove("DEEPSEEK_API_KEY")
        .output()
        .expect("the qualifier starts");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).expect("diagnostic is UTF-8");
    assert!(diagnostic.contains("--tokenizer-json"));
    assert!(!diagnostic.contains("AEX_LIVE_PROVIDER_KEY_DEEPSEEK"));
    assert!(!diagnostic.contains("DEEPSEEK_API_KEY"));
}
