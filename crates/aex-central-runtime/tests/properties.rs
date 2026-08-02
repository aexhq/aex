//! Properties the pepper keystore holds over arbitrary rotation histories.
//!
//! The scripted suites fix single cases. These fix the two invariants that must
//! hold no matter how many rotations a plane has been through: the process
//! never accumulates unbounded key material, and a version always resolves to
//! *its own* material rather than a neighbour's. The second is the one a
//! caching bug breaks silently — every credential minted under the confused
//! version stops verifying, and the platform reports them as invalid.

mod support;

use std::sync::Arc;

use aex_central_runtime::pepper::{
    PepperDirectory, PepperRecord, PepperState, SecretsManagerPepperKeystore, pepper_cache_bound,
};
use aex_identity_app::ports::{PepperKeystore as _, PepperPurpose};
use aex_identity_domain::{PepperVersion, PresentedDigest, verifier};
use base64::Engine as _;
use proptest::prelude::*;
use support::{FakeDirectory, SECRET_ID, payload, run, secret_response, secrets_client};

/// The Secrets Manager version id a pepper version is stored under.
fn version_id(version: u16) -> String {
    format!("00000000-0000-0000-0000-{version:012}")
}

/// One rotation history: distinct versions, each with distinct material.
fn history() -> impl Strategy<Value = Vec<(u16, [u8; 32])>> {
    proptest::collection::vec(any::<u8>(), 1..=24).prop_map(|seeds| {
        seeds
            .into_iter()
            .enumerate()
            .map(|(index, seed)| {
                let version = u16::try_from(index + 1).unwrap_or(1);
                // Distinct material per version: the seed varies the tail, the
                // version varies the head, so no two rows can collide.
                let mut material = [seed; 32];
                material[0] = u8::try_from(version % 251).unwrap_or(0);
                material[1] = seed.wrapping_add(1);
                (version, material)
            })
            .collect()
    })
}

proptest! {
    #[test]
    fn a_process_never_holds_more_material_than_its_bound_however_many_rotations_it_has_seen(
        history in history()
    ) {
        let rows: Vec<PepperRecord> = history
            .iter()
            .enumerate()
            .map(|(index, (version, _))| PepperRecord {
                version: PepperVersion::new(*version),
                purpose: PepperPurpose::Identity,
                // The newest row is the active one; every earlier row is
                // retiring, which is exactly the shape a rotation leaves.
                state: if index + 1 == history.len() {
                    PepperState::Active
                } else {
                    PepperState::Retiring
                },
                secret_ref: version_id(*version),
            })
            .collect();
        let answers: Vec<(u16, String)> = history
            .iter()
            .map(|(version, material)| {
                (
                    200,
                    secret_response(
                        &version_id(*version),
                        &payload(
                            u32::from(*version),
                            "identity",
                            &base64::engine::general_purpose::STANDARD.encode(material),
                        ),
                    ),
                )
            })
            .collect();
        let (client, _replay) = secrets_client(answers);
        let store = SecretsManagerPepperKeystore::new(
            client,
            SECRET_ID,
            FakeDirectory::with(rows) as Arc<dyn PepperDirectory>,
        );

        // Each version is asked for exactly once, in the order the answers were
        // scripted, so an extra fetch would exhaust the replay and fail.
        let digest = PresentedDigest::of("a-credential");
        for (version, material) in &history {
            let resolved = run(store.by_version(
                PepperPurpose::Identity,
                PepperVersion::new(*version),
            ))
            .expect("every live version resolves");
            let produced = *verifier(&resolved, &digest).as_bytes();
            let expected =
                *verifier(&aex_identity_domain::Pepper::new(*material), &digest).as_bytes();
            prop_assert_eq!(
                produced,
                expected,
                "version {} resolved to another version's material",
                version
            );
            prop_assert!(
                store.resolved() <= pepper_cache_bound(),
                "the process held {} versions of live key material",
                store.resolved()
            );
        }
        prop_assert_eq!(
            store.resolved(),
            history.len().min(pepper_cache_bound())
        );
    }

    #[test]
    fn resolving_one_version_twice_costs_one_fetch(seed in any::<u8>()) {
        let material = [seed; 32];
        let rows = vec![PepperRecord {
            version: PepperVersion::new(1),
            purpose: PepperPurpose::Identity,
            state: PepperState::Active,
            secret_ref: version_id(1),
        }];
        // One scripted answer, many resolutions. A second fetch exhausts it.
        let (client, _replay) = secrets_client(vec![(
            200,
            secret_response(
                &version_id(1),
                &payload(
                    1,
                    "identity",
                    &base64::engine::general_purpose::STANDARD.encode(material),
                ),
            ),
        )]);
        let store = SecretsManagerPepperKeystore::new(
            client,
            SECRET_ID,
            FakeDirectory::with(rows) as Arc<dyn PepperDirectory>,
        );
        let digest = PresentedDigest::of("a-credential");
        let expected =
            *verifier(&aex_identity_domain::Pepper::new(material), &digest).as_bytes();
        for _ in 0..8 {
            let resolved = run(store.active(PepperPurpose::Identity))
                .expect("the active pepper stays resolvable")
                .1;
            prop_assert_eq!(*verifier(&resolved, &digest).as_bytes(), expected);
        }
        prop_assert_eq!(store.resolved(), 1);
    }
}
