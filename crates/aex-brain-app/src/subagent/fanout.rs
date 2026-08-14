//! Planning a spawn: deterministic ids, queue reasons, and pages that fit one transaction.
//!
//! Nothing here performs I/O or reads a clock. A spawn plan is a pure function of the
//! parent's folded state, the session's capacity and the request, which is what lets the
//! 1/5/10/12-child cases be asserted exactly rather than observed.

use aex_brain_domain::budget::{
    BudgetError, BudgetGrant, BudgetNode, DIMENSIONS, Dimension, MAX_SUBAGENT_DEPTH,
    MAX_SUBAGENTS_PER_SESSION, StructuralLimits,
};
use aex_brain_domain::child::QueuedReason;
use aex_brain_domain::commit::{SPAWN_PAGE_CHILDREN, fanout_pages};
use aex_brain_domain::fold::FoldState;
use aex_brain_domain::ids::{AgentId, FanoutIntentId, JoinId, child_agent_id};
use aex_brain_domain::wire_pending::{JoinMode, join_shards};

/// What a parent asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnRequest {
    /// How many children.
    pub count: u32,
    /// What each child is granted. A grant may only reduce what the parent holds.
    pub grant: BudgetGrant,
    /// Whether the parent resumes on the first terminal child or on all of them.
    pub mode: JoinMode,
    /// The join identity the caller minted for this group.
    pub join: JoinId,
    /// The fanout intent identity, used only when the spawn needs more than one page.
    pub intent: FanoutIntentId,
}

/// Everything outside the parent's own node that decides whether a child can start now.
///
/// These are *capacity* facts, not budget facts. Budget refuses a spawn outright; capacity
/// only decides whether the child starts immediately or waits with a visible reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionCapacity {
    /// The session's shared Brain budget node.
    pub session: BudgetNode,
    /// The session's own materialized-agent ceiling, excluding the root.
    pub materialized_limit: u64,
    /// How many agents are materialized right now.
    pub materialized: u64,
    /// Provider permits free in this process.
    pub provider_permits: u64,
    /// Hands permits free for this session generation.
    pub hands_permits: u64,
    /// Whether this tenant is inside its fair share.
    pub tenant_within_share: bool,
    /// Whether the region reported capacity.
    pub regional_capacity: bool,
    /// The depth below which the scheduler defers rather than starting immediately.
    pub eager_depth: u16,
}

impl SessionCapacity {
    /// A session with room for everything. Used where a test is about something else.
    #[must_use]
    pub const fn unconstrained(session: BudgetNode) -> Self {
        Self {
            session,
            materialized_limit: u64::MAX,
            materialized: 0,
            provider_permits: u64::MAX,
            hands_permits: u64::MAX,
            tenant_within_share: true,
            regional_capacity: true,
            eager_depth: u16::MAX,
        }
    }
}

/// One child a page will create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedChild {
    /// The deterministic identity.
    pub child: AgentId,
    /// The parent's spawn-counter value that derived it.
    pub ordinal: u32,
    /// What the parent reserves for it.
    pub grant: BudgetGrant,
    /// Which limit is binding, when the child starts queued.
    ///
    /// `None` means the scheduler may claim it immediately.
    pub queued_reason: Option<QueuedReason>,
}

/// One transaction's worth of children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnPage {
    /// Which page of the intent this is.
    pub index: u32,
    /// The children it materializes.
    pub children: Vec<PlannedChild>,
}

/// The whole spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnPlan {
    /// The join every child belongs to.
    pub join: JoinId,
    /// Whether the parent resumes on the first terminal child or on all of them.
    pub mode: JoinMode,
    /// The shard count chosen at join creation.
    pub shards: u16,
    /// The durable intent, present only when the spawn needs more than one page.
    ///
    /// A single-page fanout commits directly: an intent would be a durable row written and
    /// immediately satisfied, which costs a write and explains nothing.
    pub intent: Option<FanoutIntentId>,
    /// The pages, in commit order. No child is runnable before its page commits.
    pub pages: Vec<SpawnPage>,
    /// The parent's budget node after every reservation.
    pub parent_after: BudgetNode,
}

impl SpawnPlan {
    /// Every child the plan creates, in ordinal order.
    #[must_use]
    pub fn children(&self) -> Vec<&PlannedChild> {
        self.pages.iter().flat_map(|page| &page.children).collect()
    }

    /// How many children the plan creates.
    #[must_use]
    pub fn count(&self) -> usize {
        self.pages.iter().map(|page| page.children.len()).sum()
    }
}

/// Why a spawn could not be planned.
///
/// Each is a refusal, never a clamp: silently spawning fewer children than were asked for
/// is indistinguishable to the parent's model from the children failing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpawnError {
    /// A budget or structural invariant would break.
    #[error(transparent)]
    Budget(#[from] BudgetError),
    /// The parent is terminal, so it can spawn nothing.
    #[error("a terminal agent cannot spawn")]
    Terminal,
    /// A spawn of zero children is a caller defect, not a no-op to absorb.
    #[error("a spawn of no children is never intended")]
    Empty,
}

/// Plans `request` against `state` and `capacity`.
///
/// # Errors
///
/// [`SpawnError::Empty`] for a zero-count request, [`SpawnError::Terminal`] when the parent
/// has already finished, and [`SpawnError::Budget`] when any of the nine dimensions or
/// either structural limit would break. A budget refusal reserves nothing: the returned
/// parent node is only produced on success.
pub fn plan_spawn(
    parent_id: AgentId,
    state: &FoldState,
    capacity: &SessionCapacity,
    request: &SpawnRequest,
) -> Result<SpawnPlan, SpawnError> {
    if request.count == 0 {
        return Err(SpawnError::Empty);
    }
    if state.finish.is_some() {
        return Err(SpawnError::Terminal);
    }
    let structural = StructuralLimits {
        max_depth: state.structural.max_depth.min(MAX_SUBAGENT_DEPTH),
        max_fanout: state
            .structural
            .max_fanout
            .min(MAX_SUBAGENTS_PER_SESSION as u32),
    };
    BudgetNode::check_fanout(request.count, structural)?;
    let session_allocated = capacity
        .session
        .reserved
        .get(Dimension::TotalChildrenCreated)
        .saturating_add(capacity.session.used.get(Dimension::TotalChildrenCreated));
    if u64::from(request.count) > MAX_SUBAGENTS_PER_SESSION.saturating_sub(session_allocated) {
        return Err(SpawnError::Budget(BudgetError::Exhausted {
            dimension: Dimension::TotalChildrenCreated,
            limit: capacity
                .session
                .limit
                .get(Dimension::TotalChildrenCreated)
                .min(MAX_SUBAGENTS_PER_SESSION),
            reserved: capacity
                .session
                .reserved
                .get(Dimension::TotalChildrenCreated),
            used: capacity.session.used.get(Dimension::TotalChildrenCreated),
            wanted: u64::from(request.count),
        }));
    }

    // Reserve every grant first, on a copy. If any dimension is short, the parent is left
    // exactly as it was: a partially reserved parent would leak the difference for ever.
    let mut parent = state.budget;
    let mut reserved = Vec::with_capacity(request.count as usize);
    for offset in 0..request.count {
        let ordinal = state.spawn_ordinal.saturating_add(offset);
        let child = parent.spawn(request.grant, structural)?;
        reserved.push((ordinal, child));
    }

    let mut planned: Vec<PlannedChild> = Vec::with_capacity(reserved.len());
    let mut capacity = *capacity;
    for (ordinal, child) in reserved {
        let reason = queued_reason(&capacity, child.depth);
        if reason.is_none() {
            // A child admitted now consumes the capacity the next one is measured against,
            // so a page of 32 does not admit 32 children into 1 free slot.
            capacity.materialized = capacity.materialized.saturating_add(1);
            capacity.provider_permits = capacity.provider_permits.saturating_sub(1);
            capacity.session.used.set(
                Dimension::ActiveChildren,
                capacity
                    .session
                    .used
                    .get(Dimension::ActiveChildren)
                    .saturating_add(1),
            );
        }
        planned.push(PlannedChild {
            child: child_agent_id(parent_id, ordinal),
            ordinal,
            grant: request.grant,
            queued_reason: reason,
        });
    }

    let pages: Vec<SpawnPage> = planned
        .chunks(SPAWN_PAGE_CHILDREN as usize)
        .enumerate()
        .map(|(index, chunk)| SpawnPage {
            index: u32::try_from(index).unwrap_or(u32::MAX),
            children: chunk.to_vec(),
        })
        .collect();

    Ok(SpawnPlan {
        join: request.join,
        mode: request.mode,
        shards: join_shards(request.count as usize),
        // One page commits directly; more than one needs the intent so a crash mid-fanout
        // resumes from a durable record rather than from a guess about what committed.
        intent: (fanout_pages(request.count) > 1).then_some(request.intent),
        pages,
        parent_after: parent,
    })
}

/// Which limit, if any, keeps a child at `depth` from starting immediately.
///
/// The order is deliberate and is the order an operator should read it in: the durable
/// session budget first, then the session's own ceiling, then the process's local permits,
/// then fairness, then the region. A child reports the *first* binding limit rather than a
/// list, because the first is the one that has to move for it to start.
#[must_use]
pub fn queued_reason(capacity: &SessionCapacity, depth: u16) -> Option<QueuedReason> {
    if capacity.session.free(Dimension::ActiveChildren) == 0 {
        return Some(QueuedReason::ActiveBudget);
    }
    if capacity.materialized >= capacity.materialized_limit {
        return Some(QueuedReason::SessionActiveLimit);
    }
    if capacity.provider_permits == 0 {
        return Some(QueuedReason::ProviderPermits);
    }
    if capacity.hands_permits == 0 {
        return Some(QueuedReason::HandsPermits);
    }
    if !capacity.tenant_within_share {
        return Some(QueuedReason::TenantFairness);
    }
    if !capacity.regional_capacity {
        return Some(QueuedReason::RegionalCapacity);
    }
    if depth > capacity.eager_depth {
        return Some(QueuedReason::DepthDeferred);
    }
    None
}

/// Whether `grant` only reduces what `parent` holds.
///
/// A grant that exceeded the parent's own limit would let a child outlive the ceiling its
/// parent was admitted under, which is the one thing the hierarchy exists to prevent.
#[must_use]
pub fn grant_only_reduces(parent: &BudgetNode, grant: BudgetGrant) -> bool {
    DIMENSIONS
        .iter()
        .all(|dimension| grant.get(*dimension) <= parent.limit.get(*dimension))
}

/// The structural limits a fresh session starts from.
#[must_use]
pub const fn launch_structural() -> StructuralLimits {
    StructuralLimits {
        max_depth: MAX_SUBAGENT_DEPTH,
        max_fanout: MAX_SUBAGENTS_PER_SESSION as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PlannedChild, SessionCapacity, SpawnError, SpawnRequest, plan_spawn, queued_reason,
    };
    use aex_brain_domain::budget::{BudgetNode, Dimension, DimensionVector};
    use aex_brain_domain::child::QueuedReason;
    use aex_brain_domain::commit::SPAWN_PAGE_CHILDREN;
    use aex_brain_domain::fold::FoldState;
    use aex_brain_domain::ids::{AgentId, FanoutIntentId, JoinId, child_agent_id};
    use aex_brain_domain::journal::FinishReason;
    use aex_brain_domain::wire_pending::JoinMode;
    use uuid::Uuid;

    fn parent() -> AgentId {
        AgentId(Uuid::from_u128(1))
    }

    fn folded(limit: u64) -> FoldState {
        let mut state = FoldState::empty();
        state.budget = BudgetNode::root(DimensionVector::uniform(limit));
        state
    }

    fn request(count: u32) -> SpawnRequest {
        SpawnRequest {
            count,
            grant: DimensionVector::uniform(1),
            mode: JoinMode::All,
            join: JoinId(Uuid::from_u128(9)),
            intent: FanoutIntentId(Uuid::from_u128(10)),
        }
    }

    fn capacity() -> SessionCapacity {
        SessionCapacity::unconstrained(BudgetNode::root(DimensionVector::uniform(1_000)))
    }

    /// The declared child counts, exactly. A scheduler that quietly created a different
    /// number would be indistinguishable to the parent's model from children failing.
    #[test]
    fn every_declared_child_count_produces_exactly_that_many_children() {
        for count in [1_u32, 5, 10, 12] {
            let state = folded(100_000);
            let plan = plan_spawn(parent(), &state, &capacity(), &request(count))
                .unwrap_or_else(|error| panic!("{count} children: {error}"));
            assert_eq!(plan.count(), count as usize, "{count} children");
            let ids: std::collections::BTreeSet<AgentId> =
                plan.children().iter().map(|child| child.child).collect();
            assert_eq!(ids.len(), count as usize, "{count} children are distinct");
        }
    }

    /// A retried page must create the same children, or the retry creates a second swarm.
    #[test]
    fn a_retried_plan_derives_identical_child_identities() {
        let state = folded(10_000);
        let first = plan_spawn(parent(), &state, &capacity(), &request(12)).expect("plans");
        let second = plan_spawn(parent(), &state, &capacity(), &request(12)).expect("plans");
        assert_eq!(first, second);
        assert_eq!(first.children()[0].child, child_agent_id(parent(), 0));
        assert_eq!(first.children()[11].child, child_agent_id(parent(), 11));
    }

    /// The lifetime ceiling is smaller than the transaction envelope, so every
    /// legal MVP fanout is one atomic page and needs no resume intent.
    #[test]
    fn every_legal_fanout_is_one_atomic_page() {
        let state = folded(100_000);
        let single = plan_spawn(parent(), &state, &capacity(), &request(12)).expect("plans");
        assert_eq!(single.pages.len(), 1);
        assert_eq!(
            single.intent, None,
            "one page needs no intent to resume from"
        );
        const { assert!(12 <= SPAWN_PAGE_CHILDREN) };
    }

    /// Capacity is consumed as the page is planned, so a page of 32 into one free slot
    /// admits one child and queues thirty-one with a reason.
    #[test]
    fn admitted_children_consume_the_capacity_the_next_one_is_measured_against() {
        let state = folded(10_000);
        let mut capacity = capacity();
        capacity.materialized_limit = 1;
        let plan = plan_spawn(parent(), &state, &capacity, &request(4)).expect("plans");
        let reasons: Vec<Option<QueuedReason>> = plan
            .children()
            .iter()
            .map(|child| child.queued_reason)
            .collect();
        assert_eq!(
            reasons,
            vec![
                None,
                Some(QueuedReason::SessionActiveLimit),
                Some(QueuedReason::SessionActiveLimit),
                Some(QueuedReason::SessionActiveLimit)
            ]
        );
    }

    /// A queued child always reports which limit is binding. An opaque delay tells the
    /// parent's model, and the operator, nothing about what has to change.
    #[test]
    fn each_of_the_nine_gates_names_itself() {
        let base = capacity();

        let mut exhausted = base;
        exhausted.session.used.set(
            Dimension::ActiveChildren,
            exhausted.session.limit.get(Dimension::ActiveChildren),
        );
        assert_eq!(
            queued_reason(&exhausted, 1),
            Some(QueuedReason::ActiveBudget)
        );

        let mut full = base;
        full.materialized_limit = 0;
        assert_eq!(
            queued_reason(&full, 1),
            Some(QueuedReason::SessionActiveLimit)
        );

        let mut provider = base;
        provider.provider_permits = 0;
        assert_eq!(
            queued_reason(&provider, 1),
            Some(QueuedReason::ProviderPermits)
        );

        let mut hands = base;
        hands.hands_permits = 0;
        assert_eq!(queued_reason(&hands, 1), Some(QueuedReason::HandsPermits));

        let mut tenant = base;
        tenant.tenant_within_share = false;
        assert_eq!(
            queued_reason(&tenant, 1),
            Some(QueuedReason::TenantFairness)
        );

        let mut region = base;
        region.regional_capacity = false;
        assert_eq!(
            queued_reason(&region, 1),
            Some(QueuedReason::RegionalCapacity)
        );

        let mut deep = base;
        deep.eager_depth = 1;
        assert_eq!(queued_reason(&deep, 2), Some(QueuedReason::DepthDeferred));

        assert_eq!(queued_reason(&base, 1), None);
    }

    /// A refused spawn reserves nothing. A parent left holding a partial reservation would
    /// leak the difference for the rest of the session.
    #[test]
    fn a_budget_refusal_leaves_the_parent_exactly_as_it_was() {
        let state = folded(3);
        let error = plan_spawn(parent(), &state, &capacity(), &request(4))
            .expect_err("four grants of one do not fit a limit of three");
        assert!(matches!(error, SpawnError::Budget(_)), "{error:?}");
        assert_eq!(state.budget.reserved, DimensionVector::ZERO);
    }

    #[test]
    fn depth_is_refused_structurally_rather_than_queued() {
        let mut state = folded(1_000);
        state.structural.max_depth = 0;
        let error = plan_spawn(parent(), &state, &capacity(), &request(1))
            .expect_err("depth one is below a maximum of zero");
        assert!(matches!(error, SpawnError::Budget(_)), "{error:?}");
    }

    #[test]
    fn a_fanout_above_the_structural_limit_is_refused_before_anything_is_reserved() {
        let mut state = folded(100_000);
        state.structural.max_fanout = 8;
        let error = plan_spawn(parent(), &state, &capacity(), &request(9))
            .expect_err("nine is above a fanout limit of eight");
        assert!(matches!(error, SpawnError::Budget(_)), "{error:?}");
    }

    #[test]
    fn a_terminal_parent_spawns_nothing_and_an_empty_spawn_is_a_defect() {
        let mut state = folded(1_000);
        state.finish = Some(FinishReason::Completed);
        assert_eq!(
            plan_spawn(parent(), &state, &capacity(), &request(1)),
            Err(SpawnError::Terminal)
        );
        assert_eq!(
            plan_spawn(parent(), &folded(1_000), &capacity(), &request(0)),
            Err(SpawnError::Empty)
        );
    }

    /// Join sharding still derives from the admitted size rather than a fixed count.
    #[test]
    fn the_join_shard_count_follows_the_measured_curve() {
        let state = folded(100_000);
        assert_eq!(
            plan_spawn(parent(), &state, &capacity(), &request(1))
                .expect("plans")
                .shards,
            1
        );
        assert_eq!(
            plan_spawn(parent(), &state, &capacity(), &request(12))
                .expect("plans")
                .shards,
            aex_brain_domain::wire_pending::join_shards(12)
        );
    }

    #[test]
    fn the_thirteenth_lifetime_identity_is_refused_even_after_completion() {
        let state = folded(100_000);
        let mut capacity = capacity();
        capacity
            .session
            .used
            .set(Dimension::TotalChildrenCreated, 12);
        let error = plan_spawn(parent(), &state, &capacity, &request(1))
            .expect_err("completed children still consume lifetime identities");
        assert!(matches!(
            error,
            SpawnError::Budget(aex_brain_domain::budget::BudgetError::Exhausted {
                dimension: Dimension::TotalChildrenCreated,
                ..
            })
        ));
    }

    #[test]
    fn depth_three_is_terminal_for_spawning_even_if_a_config_is_looser() {
        let mut state = folded(100_000);
        state.budget.depth = 2;
        state.structural.max_depth = u16::MAX;
        plan_spawn(parent(), &state, &capacity(), &request(1))
            .expect("a depth-two parent may allocate a depth-three child");
        state.budget.depth = 3;
        let error = plan_spawn(parent(), &state, &capacity(), &request(1))
            .expect_err("a depth-three agent cannot spawn");
        assert!(matches!(
            error,
            SpawnError::Budget(aex_brain_domain::budget::BudgetError::DepthExceeded {
                depth: 4,
                max_depth: 3
            })
        ));
    }

    #[test]
    fn a_looser_config_cannot_admit_thirteen_identities_at_once() {
        let mut state = folded(100_000);
        state.structural.max_fanout = u32::MAX;
        let error = plan_spawn(parent(), &state, &capacity(), &request(13))
            .expect_err("the MVP ceiling is not configurable upward");
        assert!(matches!(
            error,
            SpawnError::Budget(aex_brain_domain::budget::BudgetError::FanoutExceeded {
                requested: 13,
                max_fanout: 12
            })
        ));
    }

    #[test]
    fn ordinals_continue_from_the_parents_spawn_counter() {
        let mut state = folded(1_000);
        state.spawn_ordinal = 7;
        let plan = plan_spawn(parent(), &state, &capacity(), &request(2)).expect("plans");
        let ordinals: Vec<u32> = plan.children().iter().map(|child| child.ordinal).collect();
        assert_eq!(ordinals, vec![7, 8]);
        assert_eq!(
            plan.children()[0],
            &PlannedChild {
                child: child_agent_id(parent(), 7),
                ordinal: 7,
                grant: DimensionVector::uniform(1),
                queued_reason: None,
            }
        );
    }
}
