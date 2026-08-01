//! Property tests over arbitrary usage fact histories.
//!
//! The unit tests inside each module pin named cases. These pin the *laws*: for
//! any history a producer, a stream redelivery or an operator can construct, the
//! frontier order holds, the correction chain stays linear, the storage minute
//! accrual stays inside its declared bound, and no reconciled CPU charge ever
//! exceeds what the cgroup physically consumed.

use aex_usage_domain::correction::{
    CORRECTION_CHAIN_MAX, Correction, CorrectionError, CorrectionHead, CorrectionReason,
};
use aex_usage_domain::frontier::{AcceptedSequence, Frontier, PoisonReason};
use aex_usage_domain::identity::{
    AuthorityId, AuthorityKey, AuthorityKind, FactId, SegmentOrdinal,
};
use aex_usage_domain::interval::memory::{byte_ms, byte_ms_total};
use aex_usage_domain::interval::reconcile_cpu;
use aex_usage_domain::interval::storage::{
    MINUTE_MS, StorageCursor, StorageOwner, StorageOwnerKind, StorageSource, StorageTransition,
    accrue_storage,
};
use aex_usage_domain::measurement::Evidence;
use aex_usage_domain::meter::{Category, Meter, ObservabilityMeter};
use aex_usage_domain::quantity::{MAX_QUANTITY, Quantity};
use aex_usage_domain::wire_pending::{
    ActorRef, CaseId, RegionId, ServiceId, Timestamp, WorkspaceId,
};
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// shared builders
// ---------------------------------------------------------------------------

fn region() -> RegionId {
    RegionId::parse("eu-west-1").expect("region")
}

fn workspace() -> WorkspaceId {
    WorkspaceId::parse("ws-1").expect("workspace")
}

fn case() -> CaseId {
    CaseId::parse("case-1").expect("case")
}

fn actor() -> ActorRef {
    ActorRef::Reconciler {
        service: ServiceId::parse("provider-cost-reconciler").expect("service"),
    }
}

fn fact_id(seed: u64) -> FactId {
    AuthorityKey {
        region: region(),
        category: Category::Compute,
        kind: AuthorityKind::Activation,
        authority_id: AuthorityId::parse(&format!("a{seed}")).expect("id"),
        segment_ordinal: SegmentOrdinal::FIRST,
    }
    .fact_id()
}

fn at(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("representable")
}

fn owner() -> StorageOwner {
    StorageOwner {
        kind: StorageOwnerKind::ContentObject,
        id: AuthorityId::parse("content-1").expect("id"),
        generation: 1,
    }
}

/// One step a caller can take against a frontier.
#[derive(Debug, Clone, Copy)]
enum Step {
    Admit,
    Project,
    Publish,
    Settle,
}

fn steps() -> impl Strategy<Value = Vec<Step>> {
    prop::collection::vec(
        prop_oneof![
            Just(Step::Admit),
            Just(Step::Project),
            Just(Step::Publish),
            Just(Step::Settle),
        ],
        0..60,
    )
}

// ---------------------------------------------------------------------------
// U1 — frontier order over arbitrary histories
// ---------------------------------------------------------------------------

proptest! {
    /// `settled <= published <= projected <= accepted` after every step, for any
    /// interleaving. A refused step leaves the frontier exactly as it was, so a
    /// caller can never half-apply an advance.
    #[test]
    fn the_frontier_order_holds_after_every_step(steps in steps()) {
        let mut frontier = Frontier::empty(region(), workspace(), Category::Compute);
        prop_assert!(frontier.invariant());

        for step in steps {
            let attempt = match step {
                Step::Admit => frontier.admit(
                    frontier.accepted.next().expect("u64 is not exhausted here"),
                ),
                Step::Project => frontier.project(
                    frontier.projected.next().expect("u64 is not exhausted here"),
                ),
                Step::Publish => frontier.publish(
                    frontier.published.next().expect("u64 is not exhausted here"),
                ),
                Step::Settle => frontier.settle(
                    frontier.settled.next().expect("u64 is not exhausted here"),
                ),
            };
            match attempt {
                Ok(advanced) => {
                    prop_assert!(advanced.invariant(), "advance broke the order: {advanced:?}");
                    frontier = advanced;
                }
                Err(_) => {
                    // A refusal is total: nothing moved.
                    prop_assert!(frontier.invariant());
                }
            }
        }
    }

    /// No stage can ever skip a position, however far ahead it is driven.
    #[test]
    fn no_stage_advances_over_a_gap(jump in 2u64..500) {
        let frontier = Frontier::empty(region(), workspace(), Category::Compute);
        let target = AcceptedSequence::new(jump).expect("positive");
        prop_assert!(frontier.admit(target).is_err());

        let admitted = frontier
            .admit(AcceptedSequence::new(1).expect("one"))
            .expect("first admission");
        prop_assert!(admitted.project(target).is_err());
        prop_assert!(admitted.publish(target).is_err());
        prop_assert!(admitted.settle(target).is_err());
    }

    /// A quarantined frontier still admits — a fact is money evidence — but
    /// folds nothing. Nothing is ever skipped to unblock it.
    #[test]
    fn a_quarantined_frontier_admits_and_folds_nothing(depth in 1u64..40) {
        let mut frontier = Frontier::empty(region(), workspace(), Category::Compute);
        for position in 1..=depth {
            frontier = frontier
                .admit(AcceptedSequence::new(position).expect("positive"))
                .expect("admits");
        }
        let parked = frontier.quarantine(
            AcceptedSequence::new(depth).expect("positive"),
            PoisonReason::Undecodable,
        );
        prop_assert!(parked.is_quarantined());

        let after = parked
            .admit(AcceptedSequence::new(depth + 1).expect("positive"))
            .expect("admission survives quarantine");
        prop_assert!(after.is_quarantined());
        let one = AcceptedSequence::new(1).expect("one");
        prop_assert!(after.project(one).is_err());
        prop_assert!(after.publish(one).is_err());
        prop_assert!(after.settle(one).is_err());
        prop_assert!(after.invariant());
    }
}

// ---------------------------------------------------------------------------
// U1 — correction chains over arbitrary histories
// ---------------------------------------------------------------------------

proptest! {
    /// A chain is linear: each correction names the current head, the depth
    /// climbs by exactly one, and the target never changes.
    #[test]
    fn a_correction_chain_stays_linear(length in 0u32..CORRECTION_CHAIN_MAX) {
        let target = fact_id(0);
        let mut head = CorrectionHead::untouched(target.clone(), case());
        prop_assert_eq!(head.depth, 0);

        for step in 1..=length {
            let prior_head = (head.depth > 0).then(|| head.head.clone());
            let applied = fact_id(u64::from(step));
            let correction = Correction {
                case_id: case(),
                target: target.clone(),
                prior_head,
                reason: CorrectionReason::WrongQuantity,
                actor: actor(),
            };
            head = head.advance(&correction, &applied, false).expect("advances");
            prop_assert_eq!(head.depth, step);
            prop_assert_eq!(&head.target, &target);
            prop_assert_eq!(&head.head, &applied);
            prop_assert!(!head.voided);
        }
    }

    /// A stale `prior_head` is a conflict, never a silent fork. This is the CAS
    /// that keeps two concurrent operators from branching one target's history.
    #[test]
    fn a_stale_prior_head_is_refused(stale in 1u64..50) {
        let target = fact_id(0);
        let head = CorrectionHead::untouched(target.clone(), case())
            .advance(
                &Correction {
                    case_id: case(),
                    target: target.clone(),
                    prior_head: None,
                    reason: CorrectionReason::WrongQuantity,
                    actor: actor(),
                },
                &fact_id(1),
                false,
            )
            .expect("first correction");

        let forked = head.advance(
            &Correction {
                case_id: case(),
                target: target.clone(),
                prior_head: Some(fact_id(stale + 100)),
                reason: CorrectionReason::WrongQuantity,
                actor: actor(),
            },
            &fact_id(stale + 200),
            false,
        );
        prop_assert!(
            matches!(forked, Err(CorrectionError::HeadConflict { .. })),
            "a stale prior head must conflict, not fork"
        );
    }

    /// A void is terminal. No later correction, of any reason, revives a target.
    #[test]
    fn a_void_is_terminal_for_its_target(reason_index in 0usize..CorrectionReason::ALL.len()) {
        let target = fact_id(0);
        let voided = CorrectionHead::untouched(target.clone(), case())
            .advance(
                &Correction {
                    case_id: case(),
                    target: target.clone(),
                    prior_head: None,
                    reason: CorrectionReason::DuplicateMeasurement,
                    actor: actor(),
                },
                &fact_id(1),
                true,
            )
            .expect("voids");
        prop_assert!(voided.voided);

        let revived = voided.advance(
            &Correction {
                case_id: case(),
                target,
                prior_head: Some(voided.head.clone()),
                reason: CorrectionReason::ALL[reason_index],
                actor: actor(),
            },
            &fact_id(2),
            false,
        );
        prop_assert!(
            matches!(revived, Err(CorrectionError::TargetVoided { .. })),
            "a void is terminal"
        );
    }

    /// The depth ceiling is a hard refusal, not a wrap and not a truncation.
    #[test]
    fn the_chain_depth_ceiling_is_enforced(overshoot in 1u32..8) {
        let target = fact_id(0);
        let mut head = CorrectionHead::untouched(target.clone(), case());
        for step in 1..=CORRECTION_CHAIN_MAX {
            let prior_head = (head.depth > 0).then(|| head.head.clone());
            head = head
                .advance(
                    &Correction {
                        case_id: case(),
                        target: target.clone(),
                        prior_head,
                        reason: CorrectionReason::WrongQuantity,
                        actor: actor(),
                    },
                    &fact_id(u64::from(step)),
                    false,
                )
                .expect("advances up to the ceiling");
        }
        prop_assert_eq!(head.depth, CORRECTION_CHAIN_MAX);

        let over = head.advance(
            &Correction {
                case_id: case(),
                target,
                prior_head: Some(head.head.clone()),
                reason: CorrectionReason::WrongQuantity,
                actor: actor(),
            },
            &fact_id(u64::from(CORRECTION_CHAIN_MAX + overshoot)),
            false,
        );
        prop_assert!(
            matches!(over, Err(CorrectionError::ChainTooDeep { .. })),
            "the chain ceiling is a hard refusal"
        );
    }
}

// ---------------------------------------------------------------------------
// U2 — storage minute accrual (M-STOR-CLOSE)
// ---------------------------------------------------------------------------

/// The byte-minutes a closed measurement emitted, and whether it ceiled.
fn emitted_byte_minutes(measurement: &aex_usage_domain::measurement::Measurement) -> u128 {
    match measurement.evidence() {
        Evidence::StorageResidence { bytes, minutes, .. } => {
            u128::from(*bytes) * u128::from(*minutes)
        }
        other => panic!("a storage accrual produced {other:?}"),
    }
}

/// Drives a residence through interior transitions and a hard delete, returning
/// `(emitted byte-minutes, physical byte-milliseconds)`.
fn drive_residence(bytes: u64, gaps_ms: &[u64]) -> (u128, u128) {
    let mut now = 0i64;
    let mut emitted: u128 = 0;
    let mut physical_byte_ms: u128 = 0;

    let opened = accrue_storage(
        None,
        &owner(),
        StorageSource::S3,
        at(now),
        StorageTransition::Put { bytes },
        "commit-open",
    )
    .expect("opens");
    let mut cursor: StorageCursor = opened.cursor;

    for (index, gap) in gaps_ms.iter().enumerate() {
        let gap = i64::try_from(*gap).expect("bounded by the strategy");
        now += gap;
        physical_byte_ms += u128::from(bytes) * u128::try_from(gap).expect("non-negative");
        let outcome = accrue_storage(
            Some(&cursor),
            &owner(),
            StorageSource::S3,
            at(now),
            // Resizing to the same byte count keeps the physical integral simple
            // while still exercising an interior transition.
            StorageTransition::Resize { bytes },
            &format!("commit-{index}"),
        )
        .expect("interior transition");
        if let Some(measurement) = &outcome.closed {
            emitted += emitted_byte_minutes(measurement);
        }
        cursor = outcome.cursor;
    }

    let terminal = accrue_storage(
        Some(&cursor),
        &owner(),
        StorageSource::S3,
        at(now),
        StorageTransition::HardDelete,
        "commit-delete",
    )
    .expect("hard delete");
    if let Some(measurement) = &terminal.closed {
        emitted += emitted_byte_minutes(measurement);
    }

    (emitted, physical_byte_ms)
}

proptest! {
    /// `M-STOR-CLOSE`, asserted as a bound rather than assumed: the emitted
    /// byte-minutes never fall short of the physical residence and never exceed
    /// it by more than `bytes x 1 minute` — the single declared accounting
    /// transformation, bounded per residence.
    #[test]
    fn storage_accrual_stays_inside_the_declared_ceil_bound(
        bytes in 1u64..1_000_000,
        gaps in prop::collection::vec(0u64..250_000, 0..8),
    ) {
        let (emitted_byte_min, physical_byte_ms) = drive_residence(bytes, &gaps);

        let emitted_byte_ms = emitted_byte_min * u128::from(MINUTE_MS);
        let one_minute_of_bytes = u128::from(bytes) * u128::from(MINUTE_MS);

        prop_assert!(
            emitted_byte_ms + one_minute_of_bytes >= physical_byte_ms,
            "under-charged: emitted {emitted_byte_ms} vs physical {physical_byte_ms}"
        );
        prop_assert!(
            emitted_byte_ms <= physical_byte_ms + one_minute_of_bytes,
            "over-charged beyond the declared bound: emitted {emitted_byte_ms} \
             vs physical {physical_byte_ms} (+{one_minute_of_bytes})"
        );
    }

    /// A sealed cursor refuses every transition. A hard delete is terminal, so a
    /// late producer message can never reopen a closed residence.
    #[test]
    fn a_sealed_residence_refuses_everything(elapsed in 0u64..500_000) {
        let elapsed = i64::try_from(elapsed).expect("bounded");
        let opened = accrue_storage(
            None,
            &owner(),
            StorageSource::S3,
            at(0),
            StorageTransition::Put { bytes: 4_096 },
            "open",
        )
        .expect("opens");
        let sealed = accrue_storage(
            Some(&opened.cursor),
            &owner(),
            StorageSource::S3,
            at(elapsed),
            StorageTransition::HardDelete,
            "delete",
        )
        .expect("deletes");
        prop_assert!(sealed.cursor.sealed);

        for transition in [
            StorageTransition::Put { bytes: 1 },
            StorageTransition::Resize { bytes: 1 },
            StorageTransition::Trash,
            StorageTransition::Restore,
            StorageTransition::HardDelete,
        ] {
            let after = accrue_storage(
                Some(&sealed.cursor),
                &owner(),
                StorageSource::S3,
                at(elapsed + 1),
                transition,
                "late",
            );
            prop_assert!(after.is_err(), "a sealed residence accepted {transition:?}");
        }
    }

    /// A transition preceding the cursor is refused rather than charged
    /// backwards, so producer clock skew can never mint a negative interval.
    #[test]
    fn a_backwards_transition_is_refused(back in 1i64..600_000) {
        let opened = accrue_storage(
            None,
            &owner(),
            StorageSource::S3,
            at(back),
            StorageTransition::Put { bytes: 512 },
            "open",
        )
        .expect("opens");
        let earlier = accrue_storage(
            Some(&opened.cursor),
            &owner(),
            StorageSource::S3,
            at(0),
            StorageTransition::Resize { bytes: 512 },
            "backwards",
        );
        prop_assert!(earlier.is_err());
    }

    /// Trash is an interior transition, not a seal: recoverable bytes stay
    /// billable until hard delete.
    #[test]
    fn trash_keeps_a_residence_billable(held_ms in 0u64..400_000) {
        let held = i64::try_from(held_ms).expect("bounded");
        let opened = accrue_storage(
            None,
            &owner(),
            StorageSource::S3,
            at(0),
            StorageTransition::Put { bytes: 1_024 },
            "open",
        )
        .expect("opens");
        let trashed = accrue_storage(
            Some(&opened.cursor),
            &owner(),
            StorageSource::S3,
            at(held),
            StorageTransition::Trash,
            "trash",
        )
        .expect("trash is interior");
        prop_assert!(!trashed.cursor.sealed);
        prop_assert!(
            accrue_storage(
                Some(&trashed.cursor),
                &owner(),
                StorageSource::S3,
                at(held + 1),
                StorageTransition::Restore,
                "restore",
            )
            .is_ok()
        );
    }
}

// ---------------------------------------------------------------------------
// U2 — CPU reconciliation against the physical cap
// ---------------------------------------------------------------------------

proptest! {
    /// The physical upper bound over arbitrary attribution vectors, including
    /// the pathological over-attribution case: `charged <= physical` and
    /// `charged + platform == physical`, with floor allocation never over-summing.
    #[test]
    fn reconciled_cpu_never_exceeds_the_physical_reading(
        physical_us in 0u64..10_000_000,
        attributed in prop::collection::vec(0u64..5_000_000, 0..12),
    ) {
        let allocation = reconcile_cpu(physical_us, &attributed).expect("reconciles");

        prop_assert_eq!(allocation.charged.len(), attributed.len());
        prop_assert!(
            allocation.charged_us() <= physical_us,
            "charged {} exceeds physical {physical_us}",
            allocation.charged_us()
        );
        prop_assert_eq!(
            allocation.charged_us() + allocation.platform_us,
            physical_us,
            "charged + platform must account for the whole physical reading"
        );

        // No meter is ever charged more than it attributed to itself.
        for (charged, claimed) in allocation.charged.iter().zip(attributed.iter()) {
            prop_assert!(charged <= claimed);
        }

        let total: u128 = attributed.iter().copied().map(u128::from).sum();
        prop_assert_eq!(allocation.scaled, total > u128::from(physical_us));
    }

    /// Allocation is monotone: a meter that attributed more never receives a
    /// smaller charge than one that attributed less.
    #[test]
    fn cpu_allocation_is_monotone_in_attribution(
        physical_us in 1u64..1_000_000,
        attributed in prop::collection::vec(0u64..2_000_000, 1..10),
    ) {
        let mut attributed = attributed;
        attributed.sort_unstable();
        let allocation = reconcile_cpu(physical_us, &attributed).expect("reconciles");
        for window in allocation.charged.windows(2) {
            prop_assert!(
                window[0] <= window[1],
                "monotonicity broken: {:?}",
                allocation.charged
            );
        }
    }

    /// Unclaimed CPU is platform overhead and is never billed to anyone.
    #[test]
    fn unclaimed_cpu_is_platform_overhead(physical_us in 0u64..5_000_000, meters in 0usize..8) {
        let allocation = reconcile_cpu(physical_us, &vec![0; meters]).expect("reconciles");
        prop_assert_eq!(allocation.charged_us(), 0);
        prop_assert_eq!(allocation.platform_us, physical_us);
        prop_assert!(!allocation.scaled);
    }
}

// ---------------------------------------------------------------------------
// U2 — the byte-millisecond integral
// ---------------------------------------------------------------------------

proptest! {
    /// A resized reservation integrates to exactly the same total as the
    /// equivalent sequence of fixed reservations. There is no rounding step, so
    /// the equality is exact rather than approximate.
    #[test]
    fn the_byte_ms_integral_is_exact_across_resizes(
        segments in prop::collection::vec((0u64..1_000_000_000, 0u64..600_000), 0..12),
    ) {
        let total = byte_ms_total(&segments).expect("bounded by the strategy");
        let mut summed = Quantity::ZERO;
        for (bytes, held_ms) in &segments {
            summed = summed
                .checked_add(byte_ms(*bytes, *held_ms).expect("bounded"))
                .expect("bounded");
        }
        prop_assert_eq!(total, summed);
    }

    /// Splitting one hold into two adjacent holds of the same total duration
    /// changes nothing: the integral is additive in time.
    #[test]
    fn splitting_a_hold_in_time_preserves_the_integral(
        bytes in 0u64..1_000_000_000,
        first_ms in 0u64..300_000,
        second_ms in 0u64..300_000,
    ) {
        let whole = byte_ms(bytes, first_ms + second_ms).expect("bounded");
        let split = byte_ms_total(&[(bytes, first_ms), (bytes, second_ms)]).expect("bounded");
        prop_assert_eq!(whole, split);
    }
}

// ---------------------------------------------------------------------------
// U0 — deterministic identity
// ---------------------------------------------------------------------------

fn authority_kind() -> impl Strategy<Value = AuthorityKind> {
    prop::sample::select(AuthorityKind::ALL.to_vec())
}

fn category() -> impl Strategy<Value = Category> {
    prop::sample::select(Category::ALL.to_vec())
}

proptest! {
    /// Distinct authority keys give distinct fact identifiers, and an identical
    /// key always gives an identical one. That is the whole basis of producer
    /// retry idempotency: no extra state, no coordination.
    #[test]
    fn distinct_authority_keys_give_distinct_fact_ids(
        left_kind in authority_kind(),
        right_kind in authority_kind(),
        left_category in category(),
        right_category in category(),
        left_id in "[a-z0-9-]{1,24}",
        right_id in "[a-z0-9-]{1,24}",
        left_ordinal in 0u64..1_000,
        right_ordinal in 0u64..1_000,
    ) {
        let build = |kind, cat, id: &str, ordinal| AuthorityKey {
            region: region(),
            category: cat,
            kind,
            authority_id: AuthorityId::parse(id).expect("generated id is valid"),
            segment_ordinal: SegmentOrdinal::new(ordinal),
        };
        let left = build(left_kind, left_category, &left_id, left_ordinal);
        let right = build(right_kind, right_category, &right_id, right_ordinal);

        // Determinism: the same key always hashes the same way.
        prop_assert_eq!(left.fact_id(), left.fact_id());

        if left.canonical() == right.canonical() {
            prop_assert_eq!(left.fact_id(), right.fact_id());
        } else {
            prop_assert_ne!(
                left.fact_id(),
                right.fact_id(),
                "{} and {} collided",
                left.canonical(),
                right.canonical()
            );
        }
    }

    /// The canonical form is unambiguous: percent-encoding means no component
    /// can contribute a separator, so the field count is always exactly five and
    /// a minted identifier always re-parses.
    #[test]
    fn the_canonical_key_form_is_unambiguous(
        kind in authority_kind(),
        cat in category(),
        id in "[a-zA-Z0-9._~ #/-]{1,32}",
        ordinal in 0u64..1_000_000,
    ) {
        // `#` and `/` are refused by the identifier grammar outright, which is
        // exactly the fence this property relies on.
        let Ok(authority_id) = AuthorityId::parse(&id) else {
            prop_assert!(id.contains('#') || id.contains('/') || id.trim().is_empty());
            return Ok(());
        };
        let key = AuthorityKey {
            region: region(),
            category: cat,
            kind,
            authority_id,
            segment_ordinal: SegmentOrdinal::new(ordinal),
        };
        let canonical = key.canonical();
        prop_assert_eq!(canonical.split('/').count(), 5, "`{}`", canonical);

        let parsed = FactId::parse(key.fact_id().as_str()).expect("a minted id parses");
        prop_assert_eq!(parsed, key.fact_id());
    }
}

// ---------------------------------------------------------------------------
// U0 — quantity bounds
// ---------------------------------------------------------------------------

proptest! {
    /// Nothing above the storable ceiling is representable, so the row codec
    /// cannot silently truncate a quantity into a smaller charge.
    #[test]
    fn quantities_are_bounded_by_the_storable_ceiling(value in 0u128..=u128::MAX) {
        match Quantity::new(value) {
            Ok(quantity) => {
                prop_assert!(value <= MAX_QUANTITY);
                prop_assert_eq!(quantity.get(), value);
                prop_assert_eq!(
                    Quantity::parse(&quantity.to_string()).expect("round trip"),
                    quantity
                );
            }
            Err(_) => prop_assert!(value > MAX_QUANTITY),
        }
    }

    /// Addition is order-independent wherever it succeeds, so a fold over a fact
    /// history does not depend on how the stream batched it.
    #[test]
    fn quantity_addition_is_order_independent(
        values in prop::collection::vec(0u128..10u128.pow(30), 0..12),
    ) {
        let fold = |slice: &[u128]| -> Option<Quantity> {
            let mut total = Quantity::ZERO;
            for value in slice {
                total = total.checked_add(Quantity::new(*value).ok()?).ok()?;
            }
            Some(total)
        };
        let forward = fold(&values);
        let mut reversed = values;
        reversed.reverse();
        let backward = fold(&reversed);
        prop_assert_eq!(forward, backward);
    }
}

// ---------------------------------------------------------------------------
// priced and observability vocabularies stay disjoint
// ---------------------------------------------------------------------------

proptest! {
    /// No observability identifier is ever a priced meter and no priced meter is
    /// ever an observability identifier. That disjointness is what makes a rate
    /// lookup for a model token unconstructable rather than merely disabled.
    #[test]
    fn priced_and_observability_vocabularies_never_overlap(index in 0usize..64) {
        let meter = Meter::ALL[index % Meter::ALL.len()];
        let observed = ObservabilityMeter::ALL[index % ObservabilityMeter::ALL.len()];

        prop_assert!(meter.id() != observed.id());
        prop_assert!(meter.id().parse::<ObservabilityMeter>().is_err());
        prop_assert!(observed.id().parse::<Meter>().is_err());
        prop_assert_eq!(observed.category(), Category::Compute);
        prop_assert_eq!(meter.public().category(), meter.category());
    }
}
