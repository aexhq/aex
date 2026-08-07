//! When a follow socket reads full gap history.
//!
//! Gap history is a whole-partition query. Reading it on every idle cycle costs
//! one query per socket per cycle to learn, almost always, that nothing changed.
//! The batched frontier bundle already carries the workspace's monotonic
//! gap-append hint at no extra provider request, so this type turns that hint
//! into the one decision it is allowed to make: *is the ledger worth reading
//! this cycle?*
//!
//! # What the hint may and may not decide
//!
//! Gap history stays the only authority on gap state. The hint never contributes
//! a gap, removes one, or changes what one says. Three rules keep a missed hint
//! from becoming a missed gap:
//!
//! - **The first cycle always reads.** A socket never starts from a hint alone,
//!   so gap revisions written before the hint existed are still found.
//! - **Anything but a comparable, non-decreasing observation reads.** An
//!   unreadable row, a count that went backwards, a published count that
//!   vanished: none of these mean "unchanged", so all of them read and the
//!   defect is counted.
//! - **The recovery interval reads regardless.** A hint update lost by a writer
//!   that could not publish it delays a gap by at most that interval, never
//!   indefinitely.
//!
//! # Why proven absence is a value and not a defect
//!
//! A workspace that has never had a gap has no hint row, and that is the common
//! case. Absence is read here as the fact it is — no gap append has ever been
//! published — rather than as missing information, because the bundle drains its
//! unprocessed keys before decoding anything: a throttled row cannot arrive as
//! an absent one. Treating proven absence as unknown would re-read the whole
//! ledger every cycle for exactly the workspaces that have no gaps to find.
//! Absence still never suppresses a read on its own: the first cycle has already
//! read, and the recovery interval still expires.

use std::future::Future;
use std::time::{Duration, Instant};

use aex_observation_domain::gap::GapRecord;
use aex_observation_store_aws::gap_hint::GapAppendCount;

use crate::counters::{ReadCounter, ReadCounters};
use crate::frontier::GapChange;
use crate::reader::ReadError;

/// How long a socket trusts an unchanged hint before reading anyway.
///
/// Sixty seconds is the alpha value: long enough that an idle socket's ledger
/// reads drop by roughly the ratio of the fallback poll to this interval, short
/// enough that a hint update a writer failed to publish is a delay somebody
/// waits out rather than an outage they report.
pub const GAP_RECOVERY_INTERVAL: Duration = Duration::from_mins(1);

/// Why one cycle read full gap history.
///
/// Named rather than a boolean because these are the reasons an operator reads
/// back when the ledger is being read more often than expected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GapReadReason {
    /// This socket has not read the ledger yet.
    FirstCycle,
    /// The published gap-append count moved.
    HintAdvanced,
    /// The hint row could not be read, so "unchanged" is not a fact this cycle
    /// holds.
    HintUnreadable,
    /// The published count moved backwards, which a monotonic counter may not
    /// do, so nothing about it can be trusted this cycle.
    HintRegressed,
    /// A wake arrived. Wakes are untyped scope hints, so any of them may be the
    /// gap the ledger is about to show.
    Wake,
    /// The recovery interval expired.
    RecoveryInterval,
}

/// What one cycle should do about gap history.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GapHistoryDecision {
    /// Reuse the gap set the previous read returned.
    Reuse,
    /// Read the full ledger, for this reason.
    Read(GapReadReason),
}

/// What woke one cycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CycleTrigger {
    /// A keyed wake hint arrived before the fallback delay expired.
    Wake,
    /// The bounded fallback delay expired, or the cycle is the first one.
    Fallback,
}

/// What one cycle knows about the gap-change hint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GapObservation {
    /// The cycle re-read the frontier bundle, which carried this hint.
    Observed {
        /// What the batched hint row said.
        change: GapChange,
        /// What entered the cycle.
        trigger: CycleTrigger,
    },
    /// The cycle did not re-read the frontier because it walks one pinned
    /// snapshot, so there is no hint for it to act on.
    Pinned,
}

/// One gap-change observation a later cycle can be compared against.
///
/// Ordering is the whole point of the type: a later observation that sorts below
/// an earlier one is a regression, and a regression is never "unchanged".
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum GapMark {
    /// The workspace has never published a gap append.
    NeverAppended,
    /// The workspace has published this many gap appends.
    Appended(GapAppendCount),
}

impl GapMark {
    /// The comparable mark of one observation, if it has one.
    fn of(change: GapChange) -> Option<Self> {
        match change {
            GapChange::Unpublished => Some(Self::NeverAppended),
            GapChange::Counted(count) => Some(Self::Appended(count)),
            GapChange::Unreadable => None,
        }
    }
}

/// One socket's gap-history read schedule.
#[derive(Clone, Copy, Debug)]
pub struct GapWatch {
    recovery: Duration,
    last_mark: Option<GapMark>,
    last_read: Option<Instant>,
}

impl GapWatch {
    /// A watch that has read nothing yet, so its first decision is to read.
    #[must_use]
    pub fn new(recovery: Duration) -> Self {
        Self {
            recovery,
            last_mark: None,
            last_read: None,
        }
    }

    /// Decides this cycle and records what it decided.
    ///
    /// The observation is folded in whether or not it caused a read, so a hint
    /// that advances twice between reads still compares unequal to the mark the
    /// last read was made against.
    pub fn decide(
        &mut self,
        change: GapChange,
        trigger: CycleTrigger,
        now: Instant,
    ) -> GapHistoryDecision {
        let mark = GapMark::of(change);
        let reason = self.reason(mark, trigger, now);
        // An unreadable hint leaves the previous mark in place rather than
        // erasing it: the next readable observation must still be comparable to
        // the last one a read was made against.
        if let Some(mark) = mark {
            self.last_mark = Some(mark);
        }
        match reason {
            Some(reason) => {
                self.last_read = Some(now);
                GapHistoryDecision::Read(reason)
            }
            None => GapHistoryDecision::Reuse,
        }
    }

    /// The gap set one cycle should use, reading the ledger only when this
    /// watch says it is worth reading.
    ///
    /// The socket loop and every test of it go through here, so what "unchanged"
    /// costs is decided in exactly one place. `read` is a future the caller has
    /// built but not awaited: a reused cycle never polls it, so it never issues
    /// the query.
    ///
    /// # Errors
    ///
    /// Returns whatever the ledger read returns. A reused cycle cannot fail.
    pub async fn gap_history(
        &mut self,
        observation: GapObservation,
        counters: &ReadCounters,
        now: Instant,
        cached: Vec<GapRecord>,
        read: impl Future<Output = Result<Vec<GapRecord>, ReadError>>,
    ) -> Result<Vec<GapRecord>, ReadError> {
        let decision = match observation {
            GapObservation::Observed { change, trigger } => {
                if change == GapChange::Unreadable {
                    counters.record(ReadCounter::GapHintUnreadable);
                }
                self.decide(change, trigger, now)
            }
            GapObservation::Pinned => self.decide_pinned(now),
        };
        match decision {
            GapHistoryDecision::Reuse => Ok(cached),
            GapHistoryDecision::Read(_) => read.await,
        }
    }

    /// Decides a cycle that did not re-read the frontier bundle.
    ///
    /// A finite walk pins one snapshot for every page it returns, so the ledger
    /// the first page read is the ledger that snapshot has: later pages of the
    /// same walk have nothing to re-read. There is no hint to consult because
    /// there is no cycle that could act on it.
    pub fn decide_pinned(&mut self, now: Instant) -> GapHistoryDecision {
        if self.last_read.is_some() {
            return GapHistoryDecision::Reuse;
        }
        self.last_read = Some(now);
        GapHistoryDecision::Read(GapReadReason::FirstCycle)
    }

    /// Why this cycle must read, if it must.
    fn reason(
        &self,
        mark: Option<GapMark>,
        trigger: CycleTrigger,
        now: Instant,
    ) -> Option<GapReadReason> {
        let (Some(last_read), Some(last_mark)) = (self.last_read, self.last_mark) else {
            return Some(GapReadReason::FirstCycle);
        };
        let Some(mark) = mark else {
            return Some(GapReadReason::HintUnreadable);
        };
        if mark < last_mark {
            return Some(GapReadReason::HintRegressed);
        }
        if mark > last_mark {
            return Some(GapReadReason::HintAdvanced);
        }
        if trigger == CycleTrigger::Wake {
            return Some(GapReadReason::Wake);
        }
        (now.duration_since(last_read) >= self.recovery).then_some(GapReadReason::RecoveryInterval)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use aex_observation_store_aws::gap_hint::hint_update;
    use aex_wire::ids::{PrefixedId as _, WorkspaceId};
    use aws_sdk_dynamodb::types::AttributeValue;

    use super::{CycleTrigger, GAP_RECOVERY_INTERVAL, GapHistoryDecision, GapReadReason, GapWatch};
    use crate::frontier::GapChange;

    const POLL: Duration = Duration::from_secs(15);

    fn counted(appends: u64) -> GapChange {
        let item = std::collections::HashMap::from([
            (
                "itemType".to_owned(),
                AttributeValue::S("gap_change_hint".to_owned()),
            ),
            (
                "gapAppends".to_owned(),
                AttributeValue::N(appends.to_string()),
            ),
        ]);
        GapChange::Counted(
            aex_observation_store_aws::gap_hint::decode(&item).expect("a readable hint"),
        )
    }

    fn watch(now: Instant) -> (GapWatch, Instant) {
        let mut watch = GapWatch::new(GAP_RECOVERY_INTERVAL);
        assert_eq!(
            watch.decide(counted(1), CycleTrigger::Fallback, now),
            GapHistoryDecision::Read(GapReadReason::FirstCycle),
            "a socket never starts from a hint alone"
        );
        (watch, now)
    }

    #[test]
    fn gap_history_is_not_read_again_while_the_published_count_stands_still() {
        let (mut watch, start) = watch(Instant::now());

        for cycle in 1..=3 {
            assert_eq!(
                watch.decide(counted(1), CycleTrigger::Fallback, start + POLL * cycle),
                GapHistoryDecision::Reuse,
                "cycle {cycle} learned nothing changed and read nothing"
            );
        }
    }

    #[test]
    fn a_changed_count_is_read_once_and_then_stands_still_again() {
        let (mut watch, start) = watch(Instant::now());

        assert_eq!(
            watch.decide(counted(2), CycleTrigger::Fallback, start + POLL),
            GapHistoryDecision::Read(GapReadReason::HintAdvanced)
        );
        assert_eq!(
            watch.decide(counted(2), CycleTrigger::Fallback, start + POLL * 2),
            GapHistoryDecision::Reuse,
            "one change is one read, not a read on every later cycle"
        );
    }

    #[test]
    fn two_appends_between_two_cycles_are_still_one_comparison() {
        let (mut watch, start) = watch(Instant::now());

        assert_eq!(
            watch.decide(counted(9), CycleTrigger::Fallback, start + POLL),
            GapHistoryDecision::Read(GapReadReason::HintAdvanced),
            "the count is compared, not counted down"
        );
    }

    #[test]
    fn an_unreadable_hint_reads_the_ledger_rather_than_assuming_it_is_unchanged() {
        let (mut watch, start) = watch(Instant::now());

        assert_eq!(
            watch.decide(GapChange::Unreadable, CycleTrigger::Fallback, start + POLL),
            GapHistoryDecision::Read(GapReadReason::HintUnreadable)
        );
        assert_eq!(
            watch.decide(counted(1), CycleTrigger::Fallback, start + POLL * 2),
            GapHistoryDecision::Reuse,
            "the unreadable cycle left the last comparable mark in place"
        );
    }

    #[test]
    fn a_hint_that_goes_backwards_reads_the_ledger_and_is_not_trusted() {
        let (mut watch, start) = watch(Instant::now());

        assert_eq!(
            watch.decide(counted(0), CycleTrigger::Fallback, start + POLL),
            GapHistoryDecision::Read(GapReadReason::HintRegressed)
        );
        assert_eq!(
            watch.decide(
                GapChange::Unpublished,
                CycleTrigger::Fallback,
                start + POLL * 2
            ),
            GapHistoryDecision::Read(GapReadReason::HintRegressed),
            "a published counter that vanished went backwards"
        );
    }

    #[test]
    fn a_workspace_that_has_never_published_a_hint_still_reads_before_it_trusts_one() {
        let mut watch = GapWatch::new(GAP_RECOVERY_INTERVAL);
        let start = Instant::now();

        assert_eq!(
            watch.decide(GapChange::Unpublished, CycleTrigger::Fallback, start),
            GapHistoryDecision::Read(GapReadReason::FirstCycle),
            "absence never suppresses the read a socket has not yet made"
        );
        assert_eq!(
            watch.decide(GapChange::Unpublished, CycleTrigger::Fallback, start + POLL),
            GapHistoryDecision::Reuse
        );
        assert_eq!(
            watch.decide(counted(1), CycleTrigger::Fallback, start + POLL * 2),
            GapHistoryDecision::Read(GapReadReason::HintAdvanced),
            "the first published append is a change from never having published"
        );
    }

    #[test]
    fn a_missed_hint_update_is_recovered_by_the_fixed_interval() {
        let (mut watch, start) = watch(Instant::now());
        // The writer's hint publication failed, so the count never moves even
        // though the ledger grew. Only the interval can find that.
        let cycles = GAP_RECOVERY_INTERVAL.as_secs() / POLL.as_secs();

        for cycle in 1..cycles {
            assert_eq!(
                watch.decide(
                    counted(1),
                    CycleTrigger::Fallback,
                    start + POLL * u32::try_from(cycle).expect("small")
                ),
                GapHistoryDecision::Reuse,
                "cycle {cycle} is inside the interval"
            );
        }
        assert_eq!(
            watch.decide(
                counted(1),
                CycleTrigger::Fallback,
                start + GAP_RECOVERY_INTERVAL
            ),
            GapHistoryDecision::Read(GapReadReason::RecoveryInterval)
        );
    }

    #[test]
    fn a_wake_reads_the_ledger_immediately_even_when_the_hint_stands_still() {
        let (mut watch, start) = watch(Instant::now());

        assert_eq!(
            watch.decide(counted(1), CycleTrigger::Wake, start + POLL),
            GapHistoryDecision::Read(GapReadReason::Wake),
            "a wake is untyped, so it may be the gap the ledger is about to show"
        );
    }

    #[test]
    fn the_recovery_interval_restarts_from_the_last_read_whatever_caused_it() {
        let (mut watch, start) = watch(Instant::now());
        let half = GAP_RECOVERY_INTERVAL / 2;

        assert_eq!(
            watch.decide(counted(2), CycleTrigger::Fallback, start + half),
            GapHistoryDecision::Read(GapReadReason::HintAdvanced)
        );
        assert_eq!(
            watch.decide(
                counted(2),
                CycleTrigger::Fallback,
                start + GAP_RECOVERY_INTERVAL
            ),
            GapHistoryDecision::Reuse,
            "the change already read the ledger, so the interval starts from there"
        );
        assert_eq!(
            watch.decide(
                counted(2),
                CycleTrigger::Fallback,
                start + half + GAP_RECOVERY_INTERVAL
            ),
            GapHistoryDecision::Read(GapReadReason::RecoveryInterval)
        );
    }

    #[test]
    fn a_pinned_walk_reads_the_ledger_on_its_first_page_and_no_later_one() {
        let mut watch = GapWatch::new(GAP_RECOVERY_INTERVAL);
        let start = Instant::now();

        assert_eq!(
            watch.decide_pinned(start),
            GapHistoryDecision::Read(GapReadReason::FirstCycle)
        );
        for page in 1..=3 {
            assert_eq!(
                watch.decide_pinned(start + POLL * page),
                GapHistoryDecision::Reuse,
                "page {page} walks the same pinned snapshot"
            );
        }
    }

    #[test]
    fn the_hint_a_writer_publishes_is_the_hint_a_watch_compares() {
        // The writer's counter and the reader's mark are one contract: a change
        // the writer publishes must compare unequal for the reader.
        let workspace = WorkspaceId::parse("wsp_0000000001e40r2081040g2081").expect("workspace");
        let update = hint_update(workspace, 1);
        assert_eq!(update.sk, aex_observation_domain::keys::GAP_HINT_SK);
        assert_eq!(
            update.pk,
            aex_observation_domain::keys::gap_hint_pk(workspace)
        );

        let (mut watch, start) = watch(Instant::now());
        assert_eq!(
            watch.decide(counted(2), CycleTrigger::Fallback, start + POLL),
            GapHistoryDecision::Read(GapReadReason::HintAdvanced)
        );
    }
}
