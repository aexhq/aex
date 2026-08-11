//! Properties for the retained registered-content boundary.

use aex_content_domain::{
    ContentDigest, ContentMissing, DeletionDenial, DenialEpoch, DenialProjection, GcEpoch,
    MissingReason, NormalizedPath, Pin, PinSet, PlacementClass, ReachableFrom, Remediation,
    RetainReason, STAGED_ORPHAN_GRACE_MS, SweepCandidate, SweepDecision, placement_for,
    sweep_decision, unwrap_allowed,
};
use aex_wire::ids::{PrefixedId as _, Uuid7, WorkspaceId};
use aex_wire::types::Timestamp;
use proptest::prelude::*;

fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid7(Uuid7::compose(1_700_000_000_000, [7; 10]))
}

fn moment(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("in range")
}

fn segment() -> impl Strategy<Value = String> {
    prop::sample::select(vec![
        "a".to_owned(),
        "z9".to_owned(),
        "_x".to_owned(),
        "é".to_owned(),
        "中".to_owned(),
        "file.txt".to_owned(),
    ])
}

fn any_path() -> impl Strategy<Value = NormalizedPath> {
    prop::collection::vec(segment(), 1..4)
        .prop_map(|parts| NormalizedPath::parse(&parts.join("/")).expect("valid path"))
}

fn candidate(reachable_from: Vec<ReachableFrom>, staged_at: i64) -> SweepCandidate {
    SweepCandidate {
        workspace: workspace(),
        digest: ContentDigest::of(b"body"),
        key: None,
        staged_at: moment(staged_at),
        observed_epoch: GcEpoch(5),
        reachable_from,
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn path_normalization_is_idempotent(path in any_path()) {
        let reparsed = NormalizedPath::parse(path.as_str()).expect("idempotent");
        prop_assert_eq!(&reparsed, &path);
        prop_assert_eq!(reparsed.as_str(), path.as_str());
    }

    #[test]
    fn placement_boundary_is_monotone(length in 0_u64..200_000) {
        let class = placement_for(length);
        prop_assert_eq!(class == PlacementClass::Inline, length <= 32_768);
        if length > 0 {
            prop_assert!(placement_for(length - 1) <= class);
        }
    }

    #[test]
    fn gc_epoch_fence_is_monotone(observed in 0_u64..32, current in 0_u64..32) {
        let mut probe = candidate(Vec::new(), 0);
        probe.observed_epoch = GcEpoch(observed);
        let decision = sweep_decision(
            &probe,
            GcEpoch(current),
            &PinSet::new(),
            moment(STAGED_ORPHAN_GRACE_MS),
        );
        if observed < current {
            prop_assert_eq!(
                decision,
                SweepDecision::Retain(RetainReason::NewerEpoch { current: GcEpoch(current) })
            );
        } else {
            let is_not_newer_epoch = !matches!(
                decision,
                SweepDecision::Retain(RetainReason::NewerEpoch { .. })
            );
            prop_assert!(is_not_newer_epoch);
        }
    }
}

#[test]
fn every_retained_pin_kind_releases_cleanly() {
    let pins = [
        Pin::Registry {
            workspace: workspace(),
            kind: aex_content_domain::RegistryKind::File,
            name: aex_wire::ids::ResourceName::parse("readme").expect("valid"),
            revision: aex_content_domain::Revision::FIRST,
        },
        Pin::Grant {
            grant: aex_content_domain::GrantId(Uuid7::compose(1, [4; 10])),
            digest: ContentDigest::of(b"body"),
            expires_at: moment(STAGED_ORPHAN_GRACE_MS * 4),
        },
        Pin::Gc {
            epoch: GcEpoch(5),
            digest: ContentDigest::of(b"body"),
        },
    ];
    for pin in pins {
        let held: PinSet = [pin.clone()].into_iter().collect();
        assert_eq!(
            sweep_decision(
                &candidate(vec![ReachableFrom::Direct(pin.clone())], 0),
                GcEpoch(5),
                &held,
                moment(STAGED_ORPHAN_GRACE_MS),
            ),
            SweepDecision::Retain(pin.retain_reason()),
        );
        assert!(matches!(
            sweep_decision(
                &candidate(vec![ReachableFrom::Direct(pin)], 0),
                GcEpoch(5),
                &PinSet::new(),
                moment(STAGED_ORPHAN_GRACE_MS),
            ),
            SweepDecision::DeleteUnderFence { .. }
        ));
    }
}

#[test]
fn gc_grace_is_half_open_at_twenty_four_hours() {
    assert_eq!(
        sweep_decision(
            &candidate(Vec::new(), 0),
            GcEpoch(5),
            &PinSet::new(),
            moment(STAGED_ORPHAN_GRACE_MS - 1),
        ),
        SweepDecision::WaitGrace {
            until: moment(STAGED_ORPHAN_GRACE_MS)
        }
    );
    assert!(matches!(
        sweep_decision(
            &candidate(Vec::new(), 0),
            GcEpoch(5),
            &PinSet::new(),
            moment(STAGED_ORPHAN_GRACE_MS),
        ),
        SweepDecision::DeleteUnderFence { .. }
    ));
}

#[test]
fn workspace_denial_survives_metadata_restore() {
    let mut projection = DenialProjection::new();
    projection.apply(DeletionDenial {
        workspace: workspace(),
        epoch: DenialEpoch(4),
        recorded_at: moment(0),
    });
    assert!(unwrap_allowed(workspace(), &projection, DenialEpoch(4)).is_err());
    projection.advance_to(DenialEpoch(7));
    assert!(unwrap_allowed(workspace(), &projection, DenialEpoch(7)).is_err());
}

#[test]
fn content_missing_is_terminal_and_reuploadable() {
    for reason in [
        MissingReason::ObjectAbsent,
        MissingReason::ChecksumMismatch,
        MissingReason::PurgedByGc,
    ] {
        let missing = ContentMissing {
            workspace: workspace(),
            digest: ContentDigest::of(b"gone"),
            placement: PlacementClass::Object,
            reason,
        };
        assert!(!missing.retryable());
        assert_eq!(missing.remediation(), Remediation::Reupload);
        assert_eq!(missing.code(), aex_wire::error::ErrorCode::ContentMissing);
    }
}
