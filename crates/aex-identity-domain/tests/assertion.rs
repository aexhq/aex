//! The assertion envelope, byte by byte.
//!
//! The layout golden is a hex string rather than a snapshot library call,
//! because the envelope is a wire format: a change to it must show up as a
//! changed literal in the diff, not as an accepted snapshot.

use aex_control_domain::epoch::{Epoch, EpochSubjectKind};
use aex_control_domain::scope::{Scope, ScopeSet};
use aex_identity_domain::assertion::{
    ASSERTION_BODY_LEN, ASSERTION_ENVELOPE_LEN, ASSERTION_MAGIC, ASSERTION_MAX_LIFETIME_MS,
    ASSERTION_SIGNED_LEN, AssertedAccountState, Assertion, AssertionClaims, AssertionSigner,
    Audience, EPOCH_SLOTS, EpochProjection, EpochSlot, EpochSlots, IssueError, KeyId, LocalSigner,
    MAX_VERIFICATION_KEYS, Plane, PrincipalKind, VerificationInputs, VerificationKey,
    VerificationKeySet, VerifyError, audience_code, audience_from_code, issue, signing_bytes,
    verify,
};
use aex_internal_contracts::assertion::AssertionAudience;
use aex_wire::types::Region;
use uuid::Uuid;
use zeroize::Zeroizing;

const NOW: u64 = 1_767_225_600_000;

/// A projection that reports whatever it was told.
struct Projection(Vec<(EpochSubjectKind, Uuid, Epoch)>);

impl EpochProjection for Projection {
    fn projected(&self, kind: EpochSubjectKind, id: Uuid) -> Epoch {
        self.0
            .iter()
            .find(|(found_kind, found_id, _)| *found_kind == kind && *found_id == id)
            .map_or(Epoch::NEVER, |(_, _, epoch)| *epoch)
    }
}

fn fresh() -> Projection {
    Projection(Vec::new())
}

fn signer() -> LocalSigner {
    LocalSigner::new(
        KeyId::new(Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_00aa)),
        &Zeroizing::new([7_u8; 32]),
    )
}

fn key_set(signer: &LocalSigner) -> VerificationKeySet {
    VerificationKeySet::new(vec![VerificationKey {
        kid: signer.kid(),
        public_key: signer.public_key(),
        not_after_ms: NOW + 86_400_000,
    }])
    .expect("one key is inside the bound")
}

fn claims() -> AssertionClaims {
    AssertionClaims {
        issued_at_ms: NOW,
        expires_at_ms: NOW + ASSERTION_MAX_LIFETIME_MS,
        audience: Audience {
            plane: Plane::Prd,
            region: Region::EuWest1,
            service: AssertionAudience::RegionalSession,
        },
        principal_kind: PrincipalKind::WorkspaceKey,
        principal_id: Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0001),
        credential_binding: [0x5a_u8; 32],
        organization_id: Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0002),
        workspace_id: Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0003),
        workspace_region: Region::EuWest1,
        account_state: AssertedAccountState::Active,
        scopes: ScopeSet::of(&[Scope::SessionsRead, Scope::SessionsWrite]),
        epochs: EpochSlots::new(&[
            EpochSlot {
                kind: EpochSubjectKind::Key,
                id: Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0004),
                epoch: Epoch::new(3),
            },
            EpochSlot {
                kind: EpochSubjectKind::Workspace,
                id: Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0003),
                epoch: Epoch::new(1),
            },
            EpochSlot {
                kind: EpochSubjectKind::Account,
                id: Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0002),
                epoch: Epoch::NEVER,
            },
        ])
        .expect("three subjects fit"),
    }
}

fn inputs<'a>(projection: &'a Projection, binding: &'a [u8; 32]) -> VerificationInputs<'a> {
    VerificationInputs {
        now_ms: NOW + 1,
        audience: claims().audience,
        credential_binding: binding,
        projection,
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

#[test]
fn the_envelope_is_exactly_three_hundred_and_twenty_three_bytes() {
    assert_eq!(ASSERTION_ENVELOPE_LEN, 323);
    assert_eq!(ASSERTION_BODY_LEN, 235);
    assert_eq!(ASSERTION_SIGNED_LEN, 259);
    assert_eq!(EPOCH_SLOTS, 5);
    let assertion = issue(&signer(), &claims()).expect("a valid claim set signs");
    assert_eq!(assertion.as_bytes().len(), ASSERTION_ENVELOPE_LEN);
}

/// The exact bytes a fixed claim set signs.
///
/// A wire format's golden is a literal, not a snapshot: changing the layout must
/// show up as a changed string in the diff rather than as an accepted snapshot.
const SIGNED_GOLDEN: &str = "41455841010101923f2a1c00700080000000000000aa00eb0000019b76daa8000000019b76db1d300205010101923f2a1c00700080000000000000015a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a01923f2a1c007000800000000000000201923f2a1c0070008000000000000003050100000000000180000401923f2a1c007000800000000000000400000000000000030301923f2a1c007000800000000000000300000000000000010501923f2a1c007000800000000000000200000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";

#[test]
fn the_signed_prefix_is_a_fixed_layout_golden() {
    let message = signing_bytes(signer().kid(), &claims());
    assert_eq!(message.len(), ASSERTION_SIGNED_LEN);
    assert_eq!(hex(&message), SIGNED_GOLDEN);

    // The golden is readable field by field, so a layout change is diagnosable
    // from the failure rather than only from the diff.
    assert_eq!(&SIGNED_GOLDEN[0..8], "41455841", "magic");
    assert_eq!(&SIGNED_GOLDEN[8..10], "01", "version");
    assert_eq!(&SIGNED_GOLDEN[10..12], "01", "Ed25519");
    assert_eq!(&SIGNED_GOLDEN[44..48], "00eb", "body_len = 235");
    assert_eq!(&SIGNED_GOLDEN[48 + 32..48 + 34], "02", "plane = prd");
    assert_eq!(&SIGNED_GOLDEN[48 + 34..48 + 36], "05", "region = eu-west-1");
    assert_eq!(
        &SIGNED_GOLDEN[48 + 36..48 + 38],
        "01",
        "service = session-api"
    );
    assert_eq!(
        &SIGNED_GOLDEN[48 + 38..48 + 40],
        "01",
        "principal = workspace_key"
    );
    assert_eq!(&SIGNED_GOLDEN[48 + 200..48 + 202], "05", "workspace region");
    assert_eq!(
        &SIGNED_GOLDEN[48 + 202..48 + 204],
        "01",
        "account state = active"
    );
    assert_eq!(
        &SIGNED_GOLDEN[48 + 204..48 + 220],
        "0000000000018000",
        "sessions:read | sessions:write"
    );
}

#[test]
fn the_magic_version_and_algorithm_are_pinned() {
    let assertion = issue(&signer(), &claims()).expect("signs");
    let bytes = assertion.as_bytes();
    assert_eq!(&bytes[0..4], &ASSERTION_MAGIC);
    assert_eq!(bytes[4], 1, "version");
    assert_eq!(bytes[5], 1, "Ed25519");
    assert_eq!(u16::from_be_bytes([bytes[22], bytes[23]]), 235);
}

#[test]
fn a_freshly_issued_assertion_verifies_and_returns_its_claims() {
    let signer = signer();
    let claims = claims();
    let assertion = issue(&signer, &claims).expect("signs");
    let projection = fresh();
    let decoded = verify(
        assertion.as_bytes(),
        &key_set(&signer),
        &inputs(&projection, &claims.credential_binding),
    )
    .expect("verifies");
    assert_eq!(decoded, claims);
}

#[test]
fn the_base64url_round_trip_is_exact() {
    let assertion = issue(&signer(), &claims()).expect("signs");
    let text = assertion.to_base64url();
    assert_eq!(Assertion::from_base64url(&text), Ok(assertion));
    assert_eq!(
        Assertion::from_base64url("not-an-assertion"),
        Err(VerifyError::BadLength)
    );
}

#[test]
fn a_single_bit_flipped_anywhere_fails_verification() {
    let signer = signer();
    let claims = claims();
    let assertion = issue(&signer, &claims).expect("signs");
    let keys = key_set(&signer);
    let projection = fresh();

    for index in 0..ASSERTION_ENVELOPE_LEN {
        let mut mutated = *assertion.as_bytes();
        mutated[index] ^= 0x01;
        let outcome = verify(
            &mutated,
            &keys,
            &inputs(&projection, &claims.credential_binding),
        );
        assert!(
            outcome.is_err(),
            "byte {index} was flipped and the assertion still verified"
        );
    }
}

#[test]
fn every_header_failure_is_named_before_the_signature_is_checked() {
    let signer = signer();
    let claims = claims();
    let assertion = issue(&signer, &claims).expect("signs");
    let keys = key_set(&signer);
    let projection = fresh();
    let check = |bytes: &[u8]| {
        verify(
            bytes,
            &keys,
            &inputs(&projection, &claims.credential_binding),
        )
    };

    assert_eq!(
        check(&assertion.as_bytes()[..322]),
        Err(VerifyError::BadLength)
    );

    let mut mutated = *assertion.as_bytes();
    mutated[0] = b'X';
    assert_eq!(check(&mutated), Err(VerifyError::BadMagic));

    let mut mutated = *assertion.as_bytes();
    mutated[4] = 2;
    assert_eq!(check(&mutated), Err(VerifyError::BadVersion));

    let mut mutated = *assertion.as_bytes();
    mutated[5] = 2;
    assert_eq!(check(&mutated), Err(VerifyError::BadAlgorithm));

    let mut mutated = *assertion.as_bytes();
    mutated[22..24].copy_from_slice(&236_u16.to_be_bytes());
    assert_eq!(check(&mutated), Err(VerifyError::BadBodyLength));

    let mut mutated = *assertion.as_bytes();
    mutated[6] ^= 0xff;
    assert_eq!(check(&mutated), Err(VerifyError::UnknownKid));
}

#[test]
fn an_assertion_signed_by_another_key_never_verifies() {
    let claims = claims();
    let other = LocalSigner::new(signer().kid(), &Zeroizing::new([9_u8; 32]));
    let assertion = issue(&other, &claims).expect("signs");
    let projection = fresh();
    assert_eq!(
        verify(
            assertion.as_bytes(),
            &key_set(&signer()),
            &inputs(&projection, &claims.credential_binding)
        ),
        Err(VerifyError::BadSignature)
    );
}

#[test]
fn the_thirty_second_boundary_is_exact() {
    let signer = signer();
    let claims = claims();
    let assertion = issue(&signer, &claims).expect("signs");
    let keys = key_set(&signer);
    let projection = fresh();
    let at = |now_ms: u64| {
        verify(
            assertion.as_bytes(),
            &keys,
            &VerificationInputs {
                now_ms,
                audience: claims.audience,
                credential_binding: &claims.credential_binding,
                projection: &projection,
            },
        )
    };
    assert_eq!(at(NOW - 1), Err(VerifyError::NotYetValid));
    assert!(at(NOW).is_ok(), "the issue instant is inside the window");
    assert!(
        at(NOW + ASSERTION_MAX_LIFETIME_MS - 1).is_ok(),
        "one millisecond before expiry is accepted"
    );
    assert_eq!(
        at(NOW + ASSERTION_MAX_LIFETIME_MS),
        Err(VerifyError::Expired),
        "the expiry instant itself is refused"
    );
}

#[test]
fn a_lifetime_over_thirty_seconds_is_refused_at_issue_and_at_verify() {
    let signer = signer();
    let mut claims = claims();
    claims.expires_at_ms = NOW + ASSERTION_MAX_LIFETIME_MS + 1;
    assert_eq!(issue(&signer, &claims), Err(IssueError::LifetimeTooLong));

    // A hand-built envelope proves the verifier refuses even a validly signed
    // over-long assertion, so an issuer bug cannot lengthen the window.
    let message = signing_bytes(signer.kid(), &claims);
    let signature = signer.sign(&message);
    let mut envelope = [0_u8; ASSERTION_ENVELOPE_LEN];
    envelope[..ASSERTION_SIGNED_LEN].copy_from_slice(&message);
    envelope[ASSERTION_SIGNED_LEN..].copy_from_slice(&signature);
    let projection = fresh();
    assert_eq!(
        verify(
            &envelope,
            &key_set(&signer),
            &inputs(&projection, &claims.credential_binding)
        ),
        Err(VerifyError::LifetimeTooLong)
    );
}

#[test]
fn a_non_positive_lifetime_is_refused() {
    let signer = signer();
    let mut claims = claims();
    claims.expires_at_ms = claims.issued_at_ms;
    assert_eq!(
        issue(&signer, &claims),
        Err(IssueError::LifetimeNonPositive)
    );
    claims.expires_at_ms = claims.issued_at_ms - 1;
    assert_eq!(
        issue(&signer, &claims),
        Err(IssueError::LifetimeNonPositive)
    );
}

#[test]
fn a_workspace_region_that_differs_from_the_audience_is_refused_at_issue() {
    let mut claims = claims();
    claims.workspace_region = Region::UsEast1;
    assert_eq!(issue(&signer(), &claims), Err(IssueError::RegionMismatch));
}

#[test]
fn every_audience_field_is_checked() {
    let signer = signer();
    let claims = claims();
    let assertion = issue(&signer, &claims).expect("signs");
    let keys = key_set(&signer);
    let projection = fresh();

    let elsewhere = [
        Audience {
            plane: Plane::Dev,
            ..claims.audience
        },
        Audience {
            region: Region::UsEast1,
            ..claims.audience
        },
        Audience {
            service: AssertionAudience::RegionalOtlp,
            ..claims.audience
        },
    ];
    for audience in elsewhere {
        assert_eq!(
            verify(
                assertion.as_bytes(),
                &keys,
                &VerificationInputs {
                    now_ms: NOW + 1,
                    audience,
                    credential_binding: &claims.credential_binding,
                    projection: &projection,
                }
            ),
            Err(VerifyError::AudienceMismatch),
            "{audience:?}"
        );
    }
}

#[test]
fn a_different_credential_binding_is_refused() {
    let signer = signer();
    let claims = claims();
    let assertion = issue(&signer, &claims).expect("signs");
    let projection = fresh();
    assert_eq!(
        verify(
            assertion.as_bytes(),
            &key_set(&signer),
            &inputs(&projection, &[0x5b_u8; 32])
        ),
        Err(VerifyError::CredentialBindingMismatch)
    );
}

#[test]
fn a_projection_ahead_of_a_claimed_epoch_revokes_the_assertion() {
    let signer = signer();
    let claims = claims();
    let assertion = issue(&signer, &claims).expect("signs");
    let key_subject = Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0004);
    let projection = Projection(vec![(EpochSubjectKind::Key, key_subject, Epoch::new(4))]);
    assert_eq!(
        verify(
            assertion.as_bytes(),
            &key_set(&signer),
            &inputs(&projection, &claims.credential_binding)
        ),
        Err(VerifyError::EpochStale {
            kind: EpochSubjectKind::Key,
            id: key_subject,
            claimed: 3,
            projected: 4
        })
    );
}

#[test]
fn a_projection_level_with_the_claim_still_verifies() {
    let signer = signer();
    let claims = claims();
    let assertion = issue(&signer, &claims).expect("signs");
    let key_subject = Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0004);
    let projection = Projection(vec![(EpochSubjectKind::Key, key_subject, Epoch::new(3))]);
    assert!(
        verify(
            assertion.as_bytes(),
            &key_set(&signer),
            &inputs(&projection, &claims.credential_binding)
        )
        .is_ok()
    );
}

#[test]
fn an_empty_slot_before_a_used_one_is_refused() {
    let signer = signer();
    let claims = claims();
    let mut message = signing_bytes(signer.kid(), &claims);
    // Blank the first slot while leaving the second used.
    let slot = 24 + 110;
    message[slot] = 0;
    message[slot + 1..slot + 25].fill(0);
    let signature = signer.sign(&message);
    let mut envelope = [0_u8; ASSERTION_ENVELOPE_LEN];
    envelope[..ASSERTION_SIGNED_LEN].copy_from_slice(&message);
    envelope[ASSERTION_SIGNED_LEN..].copy_from_slice(&signature);
    let projection = fresh();
    assert_eq!(
        verify(
            &envelope,
            &key_set(&signer),
            &inputs(&projection, &claims.credential_binding)
        ),
        Err(VerifyError::MalformedEpochSlots)
    );
}

#[test]
fn an_empty_slot_carrying_an_id_or_an_epoch_is_refused() {
    let signer = signer();
    let claims = claims();
    let mut message = signing_bytes(signer.kid(), &claims);
    // Slot 4 is empty; give it a non-zero epoch.
    let slot = 24 + 110 + 3 * 25;
    message[slot + 24] = 1;
    let signature = signer.sign(&message);
    let mut envelope = [0_u8; ASSERTION_ENVELOPE_LEN];
    envelope[..ASSERTION_SIGNED_LEN].copy_from_slice(&message);
    envelope[ASSERTION_SIGNED_LEN..].copy_from_slice(&signature);
    let projection = fresh();
    assert_eq!(
        verify(
            &envelope,
            &key_set(&signer),
            &inputs(&projection, &claims.credential_binding)
        ),
        Err(VerifyError::MalformedEpochSlots)
    );
}

#[test]
fn a_duplicate_epoch_subject_is_refused_at_issue_and_at_verify() {
    let slot = EpochSlot {
        kind: EpochSubjectKind::Key,
        id: Uuid::from_u128(4),
        epoch: Epoch::new(1),
    };
    assert_eq!(
        EpochSlots::new(&[slot, slot]),
        Err(IssueError::DuplicateEpochSubject)
    );
    assert_eq!(
        EpochSlots::new(&[EpochSlot::EMPTY]),
        Err(IssueError::DuplicateEpochSubject),
        "an explicit empty slot in the used list is a caller error"
    );
    assert_eq!(
        EpochSlots::new(&[slot; 6]),
        Err(IssueError::TooManyEpochSubjects)
    );
}

#[test]
fn an_unknown_discriminant_is_refused_in_every_fixed_field() {
    let signer = signer();
    let claims = claims();
    let keys = key_set(&signer);
    let projection = fresh();
    // plane, region, service, principal_kind, workspace_region, account_state.
    for offset in [16_usize, 17, 18, 19, 100, 101] {
        let mut message = signing_bytes(signer.kid(), &claims);
        message[24 + offset] = 0xff;
        let signature = signer.sign(&message);
        let mut envelope = [0_u8; ASSERTION_ENVELOPE_LEN];
        envelope[..ASSERTION_SIGNED_LEN].copy_from_slice(&message);
        envelope[ASSERTION_SIGNED_LEN..].copy_from_slice(&signature);
        assert_eq!(
            verify(
                &envelope,
                &keys,
                &inputs(&projection, &claims.credential_binding)
            ),
            Err(VerifyError::UnknownDiscriminant),
            "body offset {offset}"
        );
    }
}

#[test]
fn a_scope_bit_outside_the_registry_is_refused() {
    let signer = signer();
    let claims = claims();
    let mut message = signing_bytes(signer.kid(), &claims);
    message[24 + 102..24 + 110].copy_from_slice(&(1_u64 << 63).to_be_bytes());
    let signature = signer.sign(&message);
    let mut envelope = [0_u8; ASSERTION_ENVELOPE_LEN];
    envelope[..ASSERTION_SIGNED_LEN].copy_from_slice(&message);
    envelope[ASSERTION_SIGNED_LEN..].copy_from_slice(&signature);
    let projection = fresh();
    assert_eq!(
        verify(
            &envelope,
            &key_set(&signer),
            &inputs(&projection, &claims.credential_binding)
        ),
        Err(VerifyError::UnknownScope)
    );
}

#[test]
fn a_lapsed_verification_key_stops_being_accepted() {
    let signer = signer();
    let claims = claims();
    let assertion = issue(&signer, &claims).expect("signs");
    let retired = VerificationKeySet::new(vec![VerificationKey {
        kid: signer.kid(),
        public_key: signer.public_key(),
        not_after_ms: NOW,
    }])
    .expect("one key");
    let projection = fresh();
    assert_eq!(
        verify(
            assertion.as_bytes(),
            &retired,
            &inputs(&projection, &claims.credential_binding)
        ),
        Err(VerifyError::UnknownKid)
    );
}

#[test]
fn a_rotation_accepts_both_the_active_and_the_retiring_key() {
    let old = LocalSigner::new(KeyId::new(Uuid::from_u128(1)), &Zeroizing::new([1_u8; 32]));
    let new = LocalSigner::new(KeyId::new(Uuid::from_u128(2)), &Zeroizing::new([2_u8; 32]));
    let keys = VerificationKeySet::new(vec![
        VerificationKey {
            kid: old.kid(),
            public_key: old.public_key(),
            not_after_ms: NOW + 86_400_000,
        },
        VerificationKey {
            kid: new.kid(),
            public_key: new.public_key(),
            not_after_ms: NOW + 172_800_000,
        },
    ])
    .expect("two keys");
    let claims = claims();
    let projection = fresh();
    for signer in [&old, &new] {
        let assertion = issue(signer, &claims).expect("signs");
        assert!(
            verify(
                assertion.as_bytes(),
                &keys,
                &inputs(&projection, &claims.credential_binding)
            )
            .is_ok(),
            "both keys verify during the overlap"
        );
    }
}

#[test]
fn a_key_set_is_bounded() {
    let keys: Vec<VerificationKey> = (0..=MAX_VERIFICATION_KEYS)
        .map(|index| VerificationKey {
            kid: KeyId::new(Uuid::from_u128(index as u128)),
            public_key: [0_u8; 32],
            not_after_ms: NOW,
        })
        .collect();
    assert!(VerificationKeySet::new(keys).is_err());
}

#[test]
fn every_principal_kind_round_trips_through_the_envelope() {
    let signer = signer();
    let keys = key_set(&signer);
    let projection = fresh();
    for kind in PrincipalKind::ALL {
        let mut claims = claims();
        claims.principal_kind = kind;
        let assertion = issue(&signer, &claims).expect("signs");
        let decoded = verify(
            assertion.as_bytes(),
            &keys,
            &inputs(&projection, &claims.credential_binding),
        )
        .expect("verifies");
        assert_eq!(decoded.principal_kind, kind);
    }
}

#[test]
fn every_regional_service_and_plane_round_trips() {
    let signer = signer();
    let keys = key_set(&signer);
    let projection = fresh();
    for plane in [Plane::Dev, Plane::Prd] {
        for service in AssertionAudience::ALL {
            let mut claims = claims();
            claims.audience = Audience {
                plane,
                region: Region::EuWest1,
                service,
            };
            let assertion = issue(&signer, &claims).expect("signs");
            let decoded = verify(
                assertion.as_bytes(),
                &keys,
                &VerificationInputs {
                    now_ms: NOW + 1,
                    audience: claims.audience,
                    credential_binding: &claims.credential_binding,
                    projection: &projection,
                },
            )
            .expect("verifies");
            assert_eq!(decoded.audience, claims.audience);
        }
    }
}

#[test]
fn the_audience_codec_is_total_and_is_declaration_order() {
    // One audience vocabulary, one byte codec. The enum lives in the contract
    // crate because both `central-authz` requests carry it; the byte lives here
    // because every other byte of the layout does. This test is what stops the
    // two from drifting apart.
    for (index, audience) in AssertionAudience::ALL.into_iter().enumerate() {
        let code = audience_code(audience);
        assert_eq!(
            usize::from(code),
            index + 1,
            "`{audience:?}` is not at its declaration position"
        );
        assert_eq!(audience_from_code(code), Some(audience));
    }
    // Nothing outside the vocabulary decodes, so a forged envelope cannot name
    // an audience the platform does not have.
    assert_eq!(audience_from_code(0), None);
    for byte in 6..=u8::MAX {
        assert_eq!(audience_from_code(byte), None, "{byte}");
    }
}

#[test]
fn an_issued_envelope_round_trips_through_the_internal_exchange() {
    let signer = signer();
    let assertion = issue(&signer, &claims()).expect("signs");
    let issued = assertion
        .to_issued()
        .expect("a fixed-length envelope encodes");
    assert_eq!(
        Assertion::from_issued(&issued).expect("decodes"),
        assertion,
        "the exchange must carry the envelope unchanged"
    );
    // The transport is exactly the envelope: nothing beside it can be tampered
    // with, because there is nothing beside it.
    assert_eq!(issued.as_str(), assertion.to_base64url());
}

#[test]
fn the_signer_never_renders_its_private_key() {
    let rendered = format!("{:?}", signer());
    assert!(rendered.contains("<redacted:32 bytes>"), "{rendered}");
}
