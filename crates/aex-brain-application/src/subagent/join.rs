//! Waiting on children: event-driven, never polled.
//!
//! A parked parent holds no thread, no socket, no lease and no permit. The child's terminal
//! transaction is what wakes it, so a swarm of 200 children costs 200 durable rows and one
//! parked parent rather than one waiting task per child.
//!
//! The shard counters are a **projection**. They exist to remove the write hot spot a single
//! counter creates — a measured 1 000-leaf join produced 885 first-pass conflicts on one
//! counter against 191 on 64 — and nothing more. When a counter disagrees with membership,
//! the parent re-derives from the immutable child terminal rows, because those are the only
//! completion truth.

use aex_brain_domain::child::{JoinStatus, join_status};
use aex_brain_domain::ids::{AgentId, JoinId};
use aex_brain_domain::wire_pending::{JoinGroup, JoinMode};

/// What one wake on a join concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitOutcome {
    /// Members remain outstanding. The parent parks again without holding anything.
    Pending {
        /// How many members are still outstanding, from the immutable rows.
        outstanding: usize,
    },
    /// The waiter may resume.
    Release {
        /// Which members were terminal when it resumed.
        done: Vec<AgentId>,
    },
    /// The shard sum disagrees with the immutable rows.
    ///
    /// The parent pages the child terminal rows and re-derives. This is not an error: the
    /// counter is a projection and a projection is allowed to lag or to double-count a
    /// retried increment. Believing it would be the error.
    Rederive {
        /// What the counters summed to.
        counted: u64,
        /// What the immutable rows actually say.
        observed: usize,
    },
}

/// The wake key a child terminal inserts for its parent.
///
/// `Any` keys on the child, so the first terminal wakes the parent exactly once and later
/// terminals collapse onto their own keys. `All` keys on the join, so only the writer that
/// observes the last member creates it and the parent is woken once.
#[must_use]
pub fn wake_key(join: JoinId, mode: JoinMode, child: AgentId) -> String {
    match mode {
        JoinMode::Any => format!(
            "join:{}:{}",
            join.0.as_hyphenated(),
            child.0.as_hyphenated()
        ),
        JoinMode::All => format!("join:{}:complete", join.0.as_hyphenated()),
    }
}

/// Whether a child's terminal should insert a wake for the waiting parent.
///
/// For `All` this is true only when the shard sum reaches the member count **on this
/// writer's read**, so the last writer creates the wake. A duplicate is harmless: the wake
/// dedup key collapses it.
#[must_use]
pub fn should_wake(mode: JoinMode, counted: u64, members: usize) -> bool {
    match mode {
        JoinMode::Any => true,
        JoinMode::All => counted >= members as u64,
    }
}

/// Opens a wait on `members`.
///
/// # Errors
///
/// Never. It is fallible-looking on purpose nowhere: a wait over zero members is a
/// [`WaitOutcome::Release`] immediately, because "wait for nothing" is satisfied.
#[must_use]
pub fn plan_wait(join: JoinId, mode: JoinMode, members: Vec<AgentId>) -> JoinGroup {
    let shards = aex_brain_domain::wire_pending::join_shards(members.len());
    JoinGroup {
        join,
        mode,
        done: Vec::new(),
        members,
        shards,
    }
}

/// Decides what a wake on `group` means, given what the shard counters summed to.
///
/// `counted` is the sum the parent read from the shard rows; `group.done` is what the
/// immutable child terminal rows say. When they disagree the answer is
/// [`WaitOutcome::Rederive`], never a release: releasing on a counter would let a
/// double-counted retry satisfy a join whose members are still running.
#[must_use]
pub fn resolve_wait(group: &JoinGroup, counted: u64) -> WaitOutcome {
    let observed = group
        .members
        .iter()
        .filter(|member| group.done.contains(member))
        .count();
    if counted != observed as u64 {
        return WaitOutcome::Rederive { counted, observed };
    }
    match join_status(group) {
        JoinStatus::Satisfied => WaitOutcome::Release {
            done: group
                .members
                .iter()
                .filter(|member| group.done.contains(member))
                .copied()
                .collect(),
        },
        JoinStatus::Pending => WaitOutcome::Pending {
            outstanding: group.members.len().saturating_sub(observed),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{WaitOutcome, plan_wait, resolve_wait, should_wake, wake_key};
    use aex_brain_domain::ids::{AgentId, JoinId};
    use aex_brain_domain::wire_pending::JoinMode;
    use uuid::Uuid;

    fn members(count: usize) -> Vec<AgentId> {
        (0..count)
            .map(|index| AgentId(Uuid::from_u128(index as u128)))
            .collect()
    }

    fn join() -> JoinId {
        JoinId(Uuid::from_u128(999))
    }

    #[test]
    fn any_releases_on_the_first_terminal_member() {
        let mut group = plan_wait(join(), JoinMode::Any, members(4));
        assert_eq!(
            resolve_wait(&group, 0),
            WaitOutcome::Pending { outstanding: 4 }
        );
        group.done.push(group.members[2]);
        assert_eq!(
            resolve_wait(&group, 1),
            WaitOutcome::Release {
                done: vec![group.members[2]]
            }
        );
    }

    #[test]
    fn all_releases_only_when_every_member_is_terminal() {
        let mut group = plan_wait(join(), JoinMode::All, members(3));
        group.done.push(group.members[0]);
        group.done.push(group.members[1]);
        assert_eq!(
            resolve_wait(&group, 2),
            WaitOutcome::Pending { outstanding: 1 }
        );
        group.done.push(group.members[2]);
        assert!(matches!(
            resolve_wait(&group, 3),
            WaitOutcome::Release { .. }
        ));
    }

    /// The decisive one. A counter that double-counted a retried increment must not satisfy
    /// a join whose members are still running.
    #[test]
    fn a_counter_that_disagrees_with_the_rows_makes_the_parent_rederive() {
        let mut group = plan_wait(join(), JoinMode::All, members(3));
        group.done.push(group.members[0]);
        assert_eq!(
            resolve_wait(&group, 3),
            WaitOutcome::Rederive {
                counted: 3,
                observed: 1
            }
        );
    }

    /// A duplicate terminal for one member must not advance the join.
    #[test]
    fn duplicate_terminals_collapse_onto_one_member() {
        let mut group = plan_wait(join(), JoinMode::All, members(3));
        let repeated = group.members[0];
        group.done.push(repeated);
        group.done.push(repeated);
        assert_eq!(
            resolve_wait(&group, 1),
            WaitOutcome::Pending { outstanding: 2 }
        );
    }

    /// An `Any` wake keys on the child so the first terminal wakes the parent once; an
    /// `All` wake keys on the join so only the last writer creates it.
    #[test]
    fn the_wake_key_differs_by_mode_so_a_parent_is_woken_once() {
        let first = wake_key(join(), JoinMode::Any, AgentId(Uuid::from_u128(1)));
        let second = wake_key(join(), JoinMode::Any, AgentId(Uuid::from_u128(2)));
        assert_ne!(first, second);

        let all_first = wake_key(join(), JoinMode::All, AgentId(Uuid::from_u128(1)));
        let all_second = wake_key(join(), JoinMode::All, AgentId(Uuid::from_u128(2)));
        assert_eq!(all_first, all_second, "one key, so duplicates collapse");
    }

    #[test]
    fn an_all_join_wakes_only_when_the_shard_sum_reaches_the_membership() {
        assert!(!should_wake(JoinMode::All, 2, 3));
        assert!(should_wake(JoinMode::All, 3, 3));
        assert!(
            should_wake(JoinMode::All, 4, 3),
            "a late duplicate is harmless"
        );
        assert!(should_wake(JoinMode::Any, 0, 3));
    }

    #[test]
    fn a_wait_over_no_members_is_satisfied_immediately() {
        let group = plan_wait(join(), JoinMode::All, Vec::new());
        assert!(matches!(
            resolve_wait(&group, 0),
            WaitOutcome::Release { .. }
        ));
    }

    #[test]
    fn the_shard_count_scales_with_the_membership() {
        assert_eq!(plan_wait(join(), JoinMode::All, members(1)).shards, 1);
        assert_eq!(plan_wait(join(), JoinMode::All, members(200)).shards, 32);
    }
}
