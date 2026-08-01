//! Generated properties over the harness invariants.
//!
//! The unit tests in each module pin the named cases; these cover the shapes no
//! hand-written case enumerates: arbitrary charge sequences, arbitrary
//! record/release interleavings, arbitrary text around a canary and arbitrary
//! logical resource names.

use std::sync::Arc;

use aex_test_harness::{
    Budget, Charge, CleanupLedger, Entry, Lane, ReclaimError, Reclaimer, ResourceKind,
    SecretCanary, Terminal, TestCaseId, TestRun, TestRunId, scan_for_leaks,
};
use proptest::prelude::*;

fn resource_kind() -> impl Strategy<Value = ResourceKind> {
    prop_oneof![
        Just(ResourceKind::DynamoItem),
        Just(ResourceKind::S3Object),
        Just(ResourceKind::SqsMessage),
        Just(ResourceKind::EcsTask),
        Just(ResourceKind::Session),
    ]
}

/// A reclaimer that always succeeds, so the property is about the ledger's
/// bookkeeping rather than about any plane.
#[derive(Debug)]
struct AlwaysReclaims;

impl Reclaimer for AlwaysReclaims {
    fn reclaim(&self, _entry: &Entry) -> Result<(), ReclaimError> {
        Ok(())
    }
}

proptest! {
    /// Every minted id has the declared shape, whatever else is happening.
    #[test]
    fn a_minted_run_id_is_always_well_formed(_seed in 0_u8..64) {
        let id = TestRunId::mint();
        prop_assert!(TestRunId::is_well_formed(id.as_str()), "{id}");
    }

    /// A resource name is only ever the run prefix plus the logical name, so a
    /// janitor sweeping by prefix can never miss one.
    #[test]
    fn every_resource_name_carries_the_run_prefix(logical in "[a-z][a-z0-9-]{0,40}") {
        let run = TestRun::mint(Lane::E2e, "test-architecture", 1_000);
        let name = run.resource_name(&logical);
        prop_assert!(name.starts_with(&run.resource_prefix()), "{name}");
        prop_assert!(name.ends_with(&logical), "{name}");
    }

    /// The idempotency key is a pure function of the run and the logical key,
    /// and two logical keys never collide inside one run.
    #[test]
    fn idempotency_keys_are_deterministic_and_collision_free(
        first in "[a-z0-9:-]{1,32}",
        second in "[a-z0-9:-]{1,32}",
    ) {
        let run = TestRun::mint(Lane::Load, "brain-core", 1_000);
        prop_assert_eq!(run.idempotency_key(&first), run.idempotency_key(&first));
        if first != second {
            prop_assert_ne!(run.idempotency_key(&first), run.idempotency_key(&second));
        }
    }

    /// Spend is the saturating sum of the charges and the verdict never
    /// improves: once a run is stopped it stays stopped.
    #[test]
    fn a_budget_verdict_never_recovers(soft in 0_u64..10_000, charges in prop::collection::vec(0_u64..5_000, 0..40)) {
        let budget = Budget::new(soft);
        let mut expected: u64 = 0;
        let mut stopped = false;
        for charge in &charges {
            expected = expected.saturating_add(*charge);
            let verdict = budget.charge(*charge);
            if stopped {
                prop_assert_ne!(verdict, Charge::Allowed, "a stopped run reported Allowed again");
            }
            if verdict != Charge::Allowed {
                stopped = true;
            }
            prop_assert_eq!(budget.spent_micro_usd(), expected);
        }
        if expected > budget.kill_micro_usd() {
            prop_assert_eq!(budget.verdict_now(), Charge::Killed);
        } else if expected > soft {
            prop_assert_eq!(budget.verdict_now(), Charge::SoftExceeded);
        } else {
            prop_assert_eq!(budget.verdict_now(), Charge::Allowed);
        }
    }

    /// Residue is exactly the set of recorded, unreleased, non-retained
    /// entries, under any interleaving of records and releases.
    #[test]
    fn residue_is_exactly_what_was_never_released(
        kinds in prop::collection::vec(resource_kind(), 1..12),
        release_mask in prop::collection::vec(any::<bool>(), 12),
    ) {
        let root = tempfile::tempdir().expect("a temporary directory");
        let ledger = CleanupLedger::with_root(TestRunId::mint(), root.path().to_path_buf());
        prop_assert!(ledger.install_reclaimer(Arc::new(AlwaysReclaims)));
        let mut expected: Vec<String> = Vec::new();
        for (index, kind) in kinds.iter().enumerate() {
            let identity = format!("resource-{index}");
            ledger.record(Entry::new(
                *kind,
                identity.clone(),
                Terminal::Deleted,
                TestCaseId(format!("properties::case-{index}")),
            ));
            if release_mask.get(index).copied().unwrap_or(false) {
                prop_assert!(ledger.release(*kind, &identity).is_ok());
            } else {
                expected.push(identity);
            }
        }
        let mut residue: Vec<String> = ledger.residue().into_iter().map(|entry| entry.identity).collect();
        residue.sort();
        expected.sort();
        prop_assert_eq!(&residue, &expected);
        // The ledger is loud about residue at drop, and this property
        // deliberately leaves some, so the report is taken here rather than
        // aborting the shrinking loop.
        prop_assert_eq!(ledger.take_residue_report().len(), expected.len());
    }

    /// `reclaim_all` is a total function over any recorded set: everything a
    /// succeeding reclaimer is offered is reclaimed, in non-decreasing rank
    /// order, whatever order the resources were created in.
    #[test]
    fn reclaim_all_empties_the_ledger_in_non_decreasing_rank_order(
        kinds in prop::collection::vec(resource_kind(), 1..12),
    ) {
        let root = tempfile::tempdir().expect("a temporary directory");
        let ledger = CleanupLedger::with_root(TestRunId::mint(), root.path().to_path_buf());
        prop_assert!(ledger.install_reclaimer(Arc::new(AlwaysReclaims)));
        for (index, kind) in kinds.iter().enumerate() {
            ledger.record(Entry::new(
                *kind,
                format!("resource-{index}"),
                Terminal::Deleted,
                TestCaseId(format!("properties::case-{index}")),
            ));
        }
        let summary = ledger.reclaim_all();
        prop_assert!(summary.is_complete());
        prop_assert_eq!(summary.reclaimed.len(), kinds.len());
        prop_assert!(ledger.residue().is_empty());
        let ranks: Vec<u8> = summary
            .reclaimed
            .iter()
            .map(|entry| entry.kind.reclaim_rank())
            .collect();
        prop_assert!(ranks.windows(2).all(|pair| pair[0] <= pair[1]), "{:?}", ranks);
    }

    /// A canary is found wherever it is embedded, and the finding never carries
    /// the value it found.
    #[test]
    fn a_canary_is_always_found_and_never_echoed(
        before in "[a-zA-Z0-9 ]{0,64}",
        after in "[a-zA-Z0-9 ]{0,64}",
    ) {
        let canary = SecretCanary::mint();
        let haystack = format!("{before}{}{after}", canary.expose_for_injection());
        let findings = scan_for_leaks(&haystack, Some(&canary));
        prop_assert!(!findings.is_empty(), "the canary was not found");
        for finding in &findings {
            prop_assert!(!finding.to_string().contains(canary.expose_for_injection()));
        }
    }

    /// Text that does not contain the canary and no secret shape produces no
    /// finding, so the scanner cannot fail a clean run.
    #[test]
    fn clean_text_produces_no_finding(text in "[a-z .]{0,200}") {
        let canary = SecretCanary::mint();
        prop_assert!(scan_for_leaks(&text, Some(&canary)).is_empty(), "{text}");
    }
}

/// Every lane declares a positive TTL and a residue deadline strictly after it.
#[test]
fn every_lane_has_a_ttl_and_a_later_residue_deadline() {
    let started_at = time::OffsetDateTime::now_utc();
    for lane in Lane::ALL {
        let ttl = lane.default_ttl();
        assert!(ttl.0.is_positive(), "{lane}");
        assert!(
            ttl.residue_deadline(started_at) > started_at + ttl.0,
            "{lane}"
        );
    }
}
