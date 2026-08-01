//! Key and codec properties for `regional-secret-custody`.

mod support;

use aex_secret_custody_dynamodb::codec::{
    custody_state_of, custody_state_str, decode_binding, decode_custody_head, decode_generation,
    decode_manifest, decode_provider_credential, decode_secret, encode_binding,
    encode_custody_head, encode_generation, encode_manifest, encode_provider_credential,
    encode_secret, secret_state_of, secret_state_str,
};
use aex_secret_custody_dynamodb::keys;
use aex_secret_domain::custody::{CustodyRevision, CustodyState};
use aex_secret_domain::revocation::RevocationEpoch;
use aex_secret_domain::secret::{SecretRevision, SecretState, SourceGeneration};
use proptest::prelude::*;

use support::{
    custody_head, entry, generation, manifest, metadata, now, provider_credential, session,
    workspace,
};

proptest! {
    #[test]
    fn a_secret_metadata_row_round_trips_over_arbitrary_revisions(
        revision in 1_u64..1_000_000,
        epoch in 0_u64..1_000,
    ) {
        let mut original = metadata();
        original.revision = SecretRevision(revision);
        original.revocation_epoch = RevocationEpoch(epoch);
        let encoded = encode_secret(&original).expect("encodes");
        prop_assert_eq!(
            decode_secret(&encoded, original.workspace).expect("decodes"),
            original
        );
    }

    #[test]
    fn a_generation_round_trips_over_arbitrary_ciphertext(length in 1_usize..4_096) {
        let mut original = generation();
        original.ciphertext.ciphertext = vec![0x5a; length];
        let encoded = encode_generation(&original).expect("encodes");
        prop_assert_eq!(
            decode_generation(&encoded, original.workspace).expect("decodes"),
            original
        );
    }

    #[test]
    fn a_generation_sorts_lexicographically_as_it_sorts_numerically(
        first in 0_u64..1_000_000,
        second in 0_u64..1_000_000,
    ) {
        let earlier = keys::generation(workspace(), "k", SourceGeneration(first)).expect("a key");
        let later = keys::generation(workspace(), "k", SourceGeneration(second)).expect("a key");
        prop_assert_eq!(earlier.sk < later.sk, first < second);
    }

    #[test]
    fn a_binding_sorts_by_revision_before_it_sorts_by_name(
        first in 0_u64..1_000,
        second in 0_u64..1_000,
    ) {
        let earlier = keys::binding(session(), CustodyRevision(first), "zzz").expect("a key");
        let later = keys::binding(session(), CustodyRevision(second), "aaa").expect("a key");
        if first < second {
            prop_assert!(earlier.sk < later.sk);
        }
    }
}

#[test]
fn every_state_has_exactly_one_stable_spelling_and_parses_back() {
    for state in SecretState::ALL {
        let text = secret_state_str(state);
        assert_eq!(secret_state_of(text), Some(state));
        assert!(keys::SECRET_STATES.contains(&text));
    }
    for state in [CustodyState::Active, CustodyState::Deleted] {
        let text = custody_state_str(state);
        assert_eq!(custody_state_of(text), Some(state));
        assert!(keys::CUSTODY_STATES.contains(&text));
    }
}

#[test]
fn every_row_family_round_trips() {
    let head = custody_head();
    let encoded = encode_custody_head(&head);
    assert_eq!(
        decode_custody_head(&encoded, head.workspace).expect("decodes"),
        head
    );

    let binding = encode_binding(
        session(),
        workspace(),
        CustodyRevision::FIRST,
        &entry(),
        [3; 32],
        now(),
    )
    .expect("encodes");
    assert_eq!(
        decode_binding(&binding, workspace()).expect("decodes"),
        entry()
    );

    let manifest_row = encode_manifest(&manifest());
    assert_eq!(
        decode_manifest(&manifest_row, workspace()).expect("decodes"),
        manifest()
    );

    let credential = encode_provider_credential(&provider_credential()).expect("encodes");
    assert_eq!(
        decode_provider_credential(&credential, workspace()).expect("decodes"),
        provider_credential()
    );
}
