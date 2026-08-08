//! The redaction properties the pepper adapter must hold.
//!
//! Every case here drives one of the four places secret material has escaped
//! from adapters before: a `Debug` rendering, an error body, a log line built
//! from an error, and a telemetry attribute built from the same. The material
//! under test is a fixed, recognisable 32 bytes, so any leak is a substring
//! match rather than a judgement call.

mod support;

use std::sync::Arc;

use aex_central_aws::pepper::{PepperDirectory, PepperState, SecretsManagerPepperKeystore};
use aex_identity_app::ports::{PepperKeystore as _, PepperPurpose, StoreError};
use aex_identity_domain::{PepperVersion, PresentedDigest, verifier};
use support::{
    FakeDirectory, MATERIAL_B64, SECRET_ID, VERSION_ID, payload, run, secret_response,
    secrets_client,
};

/// The base64 of the material, and the hex of the same 32 bytes.
const MATERIAL_HEX: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

fn keystore(
    directory: Arc<FakeDirectory>,
    answers: Vec<(u16, String)>,
) -> SecretsManagerPepperKeystore {
    let (client, _replay) = secrets_client(answers);
    SecretsManagerPepperKeystore::new(client, SECRET_ID, directory as Arc<dyn PepperDirectory>)
}

/// Every rendering a leak has historically travelled through.
fn renderings(store: &SecretsManagerPepperKeystore) -> Vec<String> {
    vec![format!("{store:?}"), format!("{store:#?}")]
}

fn leaks(text: &str) -> Option<&'static str> {
    for needle in [MATERIAL_B64, MATERIAL_HEX, "AAECAwQFBgcICQoLDA0ODx"] {
        if text.contains(needle) {
            return Some("material");
        }
    }
    None
}

#[test]
fn the_keystore_debug_rendering_carries_no_material() {
    let store = keystore(
        FakeDirectory::one(1, PepperState::Active, VERSION_ID),
        vec![(
            200,
            secret_response(VERSION_ID, &payload(1, "identity", MATERIAL_B64)),
        )],
    );
    run(store.active(PepperPurpose::Identity)).expect("the active pepper");
    for rendered in renderings(&store) {
        assert!(leaks(&rendered).is_none(), "{rendered}");
        assert!(
            !rendered.contains(VERSION_ID),
            "the cache named a version id: {rendered}"
        );
    }
}

#[test]
fn a_resolved_pepper_debug_rendering_is_a_fixed_redaction() {
    let store = keystore(
        FakeDirectory::one(1, PepperState::Active, VERSION_ID),
        vec![(
            200,
            secret_response(VERSION_ID, &payload(1, "identity", MATERIAL_B64)),
        )],
    );
    let (_, pepper) = run(store.active(PepperPurpose::Identity)).expect("the active pepper");
    let rendered = format!("{pepper:?}");
    assert_eq!(rendered, "<redacted:32 bytes>");
    assert!(leaks(&rendered).is_none(), "{rendered}");
}

#[test]
fn a_verifier_computed_from_a_pepper_renders_redacted_too() {
    let store = keystore(
        FakeDirectory::one(1, PepperState::Active, VERSION_ID),
        vec![(
            200,
            secret_response(VERSION_ID, &payload(1, "identity", MATERIAL_B64)),
        )],
    );
    let (_, pepper) = run(store.active(PepperPurpose::Identity)).expect("the active pepper");
    let rendered = format!("{:?}", verifier(&pepper, &PresentedDigest::of("x")));
    assert_eq!(rendered, "<redacted:32 bytes>");
}

/// One named failure path: what Secrets Manager answers, and why it fails.
struct Case {
    name: &'static str,
    answer: (u16, String),
}

/// Every failure path, and the error each produces.
fn every_failure() -> Vec<(&'static str, StoreError)> {
    let cases = vec![
        Case {
            name: "a version mismatch",
            answer: (
                200,
                secret_response(VERSION_ID, &payload(9, "identity", MATERIAL_B64)),
            ),
        },
        Case {
            name: "a purpose mismatch",
            answer: (
                200,
                secret_response(VERSION_ID, &payload(1, "cursor", MATERIAL_B64)),
            ),
        },
        Case {
            name: "a short payload",
            answer: (
                200,
                secret_response(VERSION_ID, &payload(1, "identity", "AAEC")),
            ),
        },
        Case {
            name: "a service refusal",
            answer: (
                400,
                format!(
                    r#"{{"__type":"AccessDeniedException","message":"user is not authorized to perform secretsmanager:GetSecretValue on {SECRET_ID} with material {MATERIAL_B64}"}}"#
                ),
            ),
        },
    ];
    cases
        .into_iter()
        .map(|case| {
            let store = keystore(
                FakeDirectory::one(1, PepperState::Active, VERSION_ID),
                vec![case.answer],
            );
            let error = run(store.active(PepperPurpose::Identity)).expect_err(case.name);
            (case.name, error)
        })
        .collect()
}

#[test]
fn no_failure_path_renders_material_into_its_error() {
    for (name, error) in every_failure() {
        let displayed = format!("{error}");
        let debugged = format!("{error:?}");
        assert!(leaks(&displayed).is_none(), "{name}: {displayed}");
        assert!(leaks(&debugged).is_none(), "{name}: {debugged}");
    }
}

#[test]
fn a_vendor_message_never_crosses_the_boundary() {
    // The scripted denial quotes the material in its message, which is exactly
    // what a vendor prose message is allowed to do and exactly why the code is
    // the only thing this adapter forwards.
    let (name, error) = every_failure()
        .into_iter()
        .find(|(name, _)| *name == "a service refusal")
        .expect("the service refusal case");
    assert_eq!(error, StoreError::PermissionDenied, "{name}");
    let displayed = format!("{error}");
    assert!(!displayed.contains("not authorized"), "{displayed}");
    assert!(leaks(&displayed).is_none(), "{displayed}");
}

#[test]
fn a_telemetry_attribute_built_from_a_failure_carries_no_material() {
    for (name, error) in every_failure() {
        // The shape a deployable emits: the error rendered into one string
        // attribute. If it were ever to carry material, this is where it would.
        let attribute = format!("pepper.refusal={error}");
        assert!(leaks(&attribute).is_none(), "{name}: {attribute}");
    }
}

#[test]
fn a_successful_resolution_emits_a_version_and_never_material() {
    let store = keystore(
        FakeDirectory::one(5, PepperState::Active, VERSION_ID),
        vec![(
            200,
            secret_response(VERSION_ID, &payload(5, "identity", MATERIAL_B64)),
        )],
    );
    let version = run(store.probe(PepperPurpose::Identity)).expect("the probe loads");
    assert_eq!(version, PepperVersion::new(5));
    // A version is safe to log; it is the whole point of the lifecycle table.
    let attribute = format!("pepper.version={}", version.get());
    assert!(leaks(&attribute).is_none(), "{attribute}");
    assert_eq!(attribute, "pepper.version=5");
}
