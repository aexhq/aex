//! The nine-dimension budget algebra.
//!
//! Three kinds of dimension behave differently and must not be confused:
//!
//! - **conserved** totals are charged upward at spawn and terminal boundaries only, so no
//!   per-token or per-step write ever touches a shared item;
//! - **concurrent** dimensions are exactly conserved across the tree: what a parent
//!   reserves for a child is returned in full when that child terminates;
//! - **structural** dimensions are checked, never accumulated.
//!
//! An operation that would break an invariant is a typed error, never a clamp.

use serde::{Deserialize, Serialize};

/// The MVP lifetime ceiling for non-root agent identities in one session.
/// Completed and cancelled children still count.
pub const MAX_SUBAGENTS_PER_SESSION: u64 = 12;

/// The MVP lineage ceiling, with the root at depth zero.
pub const MAX_SUBAGENT_DEPTH: u16 = 3;

/// One accumulating budget dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dimension {
    /// `C` — children ever created below this node.
    TotalChildrenCreated,
    /// `P` — provider calls.
    ProviderCalls,
    /// `H` — Hands calls.
    HandsCalls,
    /// `$` — cost in micro-USD. Under `BYOK` a model call does not touch this.
    CostMicroUsd,
    /// `A` — children materialized at the same time.
    ActiveChildren,
    /// `Q` — children durably queued at the same time.
    QueuedChildren,
    /// `B` — retained child result bytes.
    RetainedResultBytes,
}

/// Which arithmetic a dimension obeys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Charged upward at spawn and terminal boundaries; a child's actual use rolls up.
    Conserved,
    /// Returned in full at the child's terminal; exactly conserved across the tree.
    Concurrent,
}

/// Every accumulating dimension, in a fixed order so a vector index is stable.
pub const DIMENSIONS: [Dimension; 7] = [
    Dimension::TotalChildrenCreated,
    Dimension::ProviderCalls,
    Dimension::HandsCalls,
    Dimension::CostMicroUsd,
    Dimension::ActiveChildren,
    Dimension::QueuedChildren,
    Dimension::RetainedResultBytes,
];

impl Dimension {
    /// Which arithmetic this dimension obeys.
    #[must_use]
    pub const fn kind(self) -> Kind {
        match self {
            Self::TotalChildrenCreated
            | Self::ProviderCalls
            | Self::HandsCalls
            | Self::CostMicroUsd => Kind::Conserved,
            Self::ActiveChildren | Self::QueuedChildren | Self::RetainedResultBytes => {
                Kind::Concurrent
            }
        }
    }

    /// The fixed index of this dimension.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::TotalChildrenCreated => 0,
            Self::ProviderCalls => 1,
            Self::HandsCalls => 2,
            Self::CostMicroUsd => 3,
            Self::ActiveChildren => 4,
            Self::QueuedChildren => 5,
            Self::RetainedResultBytes => 6,
        }
    }
}

/// The two structural dimensions. They are checked at spawn, never accumulated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuralLimits {
    /// `D` — deepest lineage depth admitted, counted from the root at zero.
    pub max_depth: u16,
    /// `F` — children admitted per fanout decision.
    pub max_fanout: u32,
}

impl Default for StructuralLimits {
    /// The launch defaults. Both are durable revisioned records in production, never
    /// compiled constants; these values exist so a test has a starting point.
    fn default() -> Self {
        Self {
            max_depth: MAX_SUBAGENT_DEPTH,
            max_fanout: MAX_SUBAGENTS_PER_SESSION as u32,
        }
    }
}

/// A per-dimension vector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DimensionVector(pub [u64; 7]);

impl DimensionVector {
    /// The zero vector.
    pub const ZERO: Self = Self([0; 7]);

    /// The value of `dimension`.
    #[must_use]
    pub const fn get(&self, dimension: Dimension) -> u64 {
        self.0[dimension.index()]
    }

    /// Sets `dimension` to `value`.
    pub const fn set(&mut self, dimension: Dimension, value: u64) {
        self.0[dimension.index()] = value;
    }

    /// A vector with every dimension set to `value`.
    #[must_use]
    pub const fn uniform(value: u64) -> Self {
        Self([value; 7])
    }
}

/// The limits granted to one node when it was created.
pub type BudgetGrant = DimensionVector;

/// A change to one dimension, as a decision transaction records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetDelta {
    /// Which dimension moved.
    pub dimension: Dimension,
    /// How much.
    pub quantity: u64,
}

impl BudgetDelta {
    /// `quantity` units of `dimension`.
    #[must_use]
    pub const fn new(dimension: Dimension, quantity: u64) -> Self {
        Self {
            dimension,
            quantity,
        }
    }
}

/// One node in the budget tree.
///
/// The invariants are `I1` (`reserved + used <= limit`, every node, every dimension) and
/// `I2` (a grant never exceeds the parent's free headroom at grant time).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BudgetNode {
    /// What this node may ever use.
    pub limit: DimensionVector,
    /// What this node has promised to its children.
    pub reserved: DimensionVector,
    /// What this node has actually consumed.
    pub used: DimensionVector,
    /// Lineage depth, root at zero.
    pub depth: u16,
}

/// Why a budget operation was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BudgetError {
    /// `I1` would be broken: the node has no headroom left in that dimension.
    #[error(
        "{dimension:?} exhausted: limit {limit}, reserved {reserved}, used {used}, wanted {wanted}"
    )]
    Exhausted {
        /// The dimension that has no headroom.
        dimension: Dimension,
        /// The node's limit.
        limit: u64,
        /// What the node has promised to children.
        reserved: u64,
        /// What the node has consumed.
        used: u64,
        /// What was asked for.
        wanted: u64,
    },
    /// A release asked for more than was reserved, which means the tree already diverged.
    #[error("{dimension:?} release of {wanted} exceeds the {reserved} reserved")]
    ReleaseUnderflow {
        /// The dimension.
        dimension: Dimension,
        /// What was reserved.
        reserved: u64,
        /// What the release asked to return.
        wanted: u64,
    },
    /// The child would sit deeper than the session's structural limit.
    #[error("lineage depth {depth} exceeds the {max_depth} permitted")]
    DepthExceeded {
        /// The depth the child would have had.
        depth: u16,
        /// The structural limit.
        max_depth: u16,
    },
    /// A reconciliation reported spending more than the permit reserved.
    ///
    /// Not a clamp and not a top-up: the reservation is what authorised the call,
    /// so a larger actual means the caller spent money it was never granted. The
    /// reservation stays charged in full and this is returned, because the two
    /// alternatives are worse — refunding would credit a session for money that
    /// left, and charging the excess would let a caller spend past a bound by
    /// reporting a bigger number afterwards.
    #[error("a reconciliation reported {actual} against a {reserved} reservation")]
    ReconcileOverspend {
        /// What the permit reserved.
        reserved: u64,
        /// What the caller says was spent.
        actual: u64,
    },
    /// The fanout page asked to admit more children than one decision may.
    #[error("fanout of {requested} exceeds the {max_fanout} admitted per decision")]
    FanoutExceeded {
        /// How many children the decision asked for.
        requested: u32,
        /// The structural limit.
        max_fanout: u32,
    },
}

/// Proof that a session reserved platform-paid spend before it was spent.
///
/// Unforgeable outside this module: the field is private and there is no public
/// constructor, so the only way to obtain one is
/// [`BudgetNode::reserve_spend`]. It is deliberately **not** `Clone` and **not**
/// `Copy`, because a permit that could be duplicated would be a reservation that
/// could be settled twice — once as a refund and once as a charge.
#[derive(Debug, PartialEq, Eq)]
#[must_use = "an unsettled reservation stays charged against the session for its whole life"]
pub struct SessionSpendPermit {
    reserved: u64,
}

impl SessionSpendPermit {
    /// How much this permit reserved.
    #[must_use]
    pub const fn reserved(&self) -> u64 {
        self.reserved
    }
}

impl BudgetNode {
    /// A root node with `limit` and depth zero.
    #[must_use]
    pub const fn root(limit: DimensionVector) -> Self {
        Self {
            limit,
            reserved: DimensionVector::ZERO,
            used: DimensionVector::ZERO,
            depth: 0,
        }
    }

    /// Free headroom in `dimension`: `limit - reserved - used`.
    #[must_use]
    pub const fn free(&self, dimension: Dimension) -> u64 {
        self.limit
            .get(dimension)
            .saturating_sub(self.reserved.get(dimension))
            .saturating_sub(self.used.get(dimension))
    }

    /// Whether `I1` holds for every dimension.
    #[must_use]
    pub fn invariant_i1(&self) -> bool {
        DIMENSIONS.iter().all(|&dimension| {
            self.reserved
                .get(dimension)
                .saturating_add(self.used.get(dimension))
                <= self.limit.get(dimension)
        })
    }

    /// Reserves `grant` for a new child and returns the child's node.
    ///
    /// The reservation and the child's creation are one transaction in the store, which is
    /// why this is one fallible operation rather than two steps a caller could interleave.
    ///
    /// # Errors
    ///
    /// Returns [`BudgetError::Exhausted`] when `I2` would be broken in any dimension and
    /// [`BudgetError::DepthExceeded`] when the child would sit below the structural limit.
    pub fn spawn(
        &mut self,
        grant: BudgetGrant,
        structural: StructuralLimits,
    ) -> Result<Self, BudgetError> {
        let depth = self.depth.saturating_add(1);
        if depth > structural.max_depth {
            return Err(BudgetError::DepthExceeded {
                depth,
                max_depth: structural.max_depth,
            });
        }
        for &dimension in &DIMENSIONS {
            let wanted = grant.get(dimension);
            if wanted > self.free(dimension) {
                return Err(BudgetError::Exhausted {
                    dimension,
                    limit: self.limit.get(dimension),
                    reserved: self.reserved.get(dimension),
                    used: self.used.get(dimension),
                    wanted,
                });
            }
        }
        for &dimension in &DIMENSIONS {
            let reserved = self
                .reserved
                .get(dimension)
                .saturating_add(grant.get(dimension));
            self.reserved.set(dimension, reserved);
        }
        Ok(Self {
            limit: grant,
            reserved: DimensionVector::ZERO,
            used: DimensionVector::ZERO,
            depth,
        })
    }

    /// Charges `quantity` of `dimension` to this node.
    ///
    /// # Errors
    ///
    /// Returns [`BudgetError::Exhausted`] when `I1` would be broken.
    pub fn consume(&mut self, dimension: Dimension, quantity: u64) -> Result<(), BudgetError> {
        if quantity > self.free(dimension) {
            return Err(BudgetError::Exhausted {
                dimension,
                limit: self.limit.get(dimension),
                reserved: self.reserved.get(dimension),
                used: self.used.get(dimension),
                wanted: quantity,
            });
        }
        let used = self.used.get(dimension).saturating_add(quantity);
        self.used.set(dimension, used);
        Ok(())
    }

    /// Reserves `estimate` micro-USD of platform-paid spend and proves it.
    ///
    /// **The permit is the mechanism.** [`BudgetNode::reconcile_spend`] requires
    /// one *by value*, so a spend cannot be settled without a reservation and a
    /// reservation cannot be settled twice. `SessionSpendPermit` has a private
    /// field and no public constructor, so nothing outside this module can make
    /// one — the same trick `DispatchTicket` plays with `FenceGuard` and
    /// `Grant<C>` plays in the regional composition, and it is here for the same
    /// reason: a check a caller can forget is a check that will be forgotten.
    ///
    /// Reserving *before* the call rather than charging after it is the other
    /// half. A session that only charged on settlement could overshoot its whole
    /// budget by its concurrency width, because every call in flight would see
    /// the headroom the others had not yet spent.
    ///
    /// # Errors
    ///
    /// Returns [`BudgetError::Exhausted`] when the session has no headroom left
    /// in [`Dimension::CostMicroUsd`], which is what produces a clean
    /// `FinishReason::Budget` rather than an opaque dispatch failure.
    pub fn reserve_spend(&mut self, estimate: u64) -> Result<SessionSpendPermit, BudgetError> {
        self.consume(Dimension::CostMicroUsd, estimate)?;
        Ok(SessionSpendPermit { reserved: estimate })
    }

    /// Settles a reservation against what was actually spent.
    ///
    /// Takes the permit by value so one reservation settles exactly once.
    ///
    /// # Errors
    ///
    /// Returns [`BudgetError::ReconcileOverspend`] when `actual` exceeds what the
    /// permit reserved. The reservation stays charged in full: the money left, and
    /// a caller must not be able to raise its own ceiling by reporting a larger
    /// number after the fact.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "taking the permit by value is the mechanism: one reservation settles                   exactly once, and a borrowed permit could settle twice"
    )]
    pub fn reconcile_spend(
        &mut self,
        permit: SessionSpendPermit,
        actual: u64,
    ) -> Result<(), BudgetError> {
        // Destructured rather than read through the binding, so the permit is
        // consumed here and cannot settle a second time.
        let SessionSpendPermit { reserved } = permit;
        if actual > reserved {
            return Err(BudgetError::ReconcileOverspend { reserved, actual });
        }
        let refund = reserved - actual;
        let used = self
            .used
            .get(Dimension::CostMicroUsd)
            .saturating_sub(refund);
        self.used.set(Dimension::CostMicroUsd, used);
        Ok(())
    }

    /// Releases a terminal child back into this node.
    ///
    /// Concurrent dimensions return in full; conserved dimensions return the unspent
    /// remainder and roll the child's actual use up. That is what makes unspent conserved
    /// budget flow back to the parent automatically.
    ///
    /// # Errors
    ///
    /// Returns [`BudgetError::ReleaseUnderflow`] when the child's grant was never reserved
    /// here, which means the tree already diverged and must not be patched over.
    pub fn release_child(&mut self, child: &Self) -> Result<(), BudgetError> {
        for &dimension in &DIMENSIONS {
            let granted = child.limit.get(dimension);
            let reserved = self.reserved.get(dimension);
            if granted > reserved {
                return Err(BudgetError::ReleaseUnderflow {
                    dimension,
                    reserved,
                    wanted: granted,
                });
            }
        }
        for &dimension in &DIMENSIONS {
            let granted = child.limit.get(dimension);
            self.reserved
                .set(dimension, self.reserved.get(dimension) - granted);
            if dimension.kind() == Kind::Conserved {
                let rolled = self
                    .used
                    .get(dimension)
                    .saturating_add(child.used.get(dimension));
                self.used.set(dimension, rolled);
            }
        }
        Ok(())
    }

    /// Checks a fanout page against the structural fanout limit.
    ///
    /// # Errors
    ///
    /// Returns [`BudgetError::FanoutExceeded`] when the page is larger than one decision
    /// may admit.
    pub const fn check_fanout(
        requested: u32,
        structural: StructuralLimits,
    ) -> Result<(), BudgetError> {
        if requested > structural.max_fanout {
            return Err(BudgetError::FanoutExceeded {
                requested,
                max_fanout: structural.max_fanout,
            });
        }
        Ok(())
    }
}

/// The launch default session grant.
///
/// Every value here is a durable revisioned limit record with an override in production.
/// It is a constant only so a test has a starting point; the Slice 11 load gates are the
/// sizing authority that confirms or replaces it.
#[must_use]
pub fn launch_session_grant(cost_micro_usd: u64) -> BudgetGrant {
    let mut grant = DimensionVector::ZERO;
    grant.set(
        Dimension::TotalChildrenCreated,
        MAX_SUBAGENTS_PER_SESSION,
    );
    grant.set(Dimension::ProviderCalls, u64::MAX);
    grant.set(Dimension::HandsCalls, 4_096);
    grant.set(Dimension::CostMicroUsd, cost_micro_usd);
    grant.set(Dimension::ActiveChildren, MAX_SUBAGENTS_PER_SESSION);
    grant.set(Dimension::QueuedChildren, MAX_SUBAGENTS_PER_SESSION);
    grant.set(Dimension::RetainedResultBytes, 256 * 1_024 * 1_024);
    grant
}

#[cfg(test)]
mod tests {
    use super::{
        BudgetError, BudgetNode, DIMENSIONS, Dimension, DimensionVector, Kind, StructuralLimits,
        launch_session_grant,
    };

    fn parent() -> BudgetNode {
        BudgetNode::root(DimensionVector::uniform(100))
    }

    #[test]
    fn every_dimension_has_a_distinct_stable_index() {
        let mut indices: Vec<usize> = DIMENSIONS.iter().map(|d| d.index()).collect();
        indices.sort_unstable();
        assert_eq!(indices, (0..DIMENSIONS.len()).collect::<Vec<_>>());
    }

    #[test]
    fn a_grant_may_not_exceed_the_parents_free_headroom() {
        let mut node = parent();
        let error = node
            .spawn(DimensionVector::uniform(101), StructuralLimits::default())
            .expect_err("an over-large grant is refused");
        assert!(matches!(error, BudgetError::Exhausted { .. }), "{error:?}");
        assert_eq!(
            node.reserved,
            DimensionVector::ZERO,
            "a refusal reserves nothing"
        );
    }

    #[test]
    fn concurrent_budget_is_exactly_conserved_across_a_child_lifetime() {
        let mut node = parent();
        let before = node.reserved.get(Dimension::ActiveChildren);
        let child = node
            .spawn(DimensionVector::uniform(10), StructuralLimits::default())
            .expect("a grant inside headroom");
        assert_eq!(node.reserved.get(Dimension::ActiveChildren), before + 10);
        node.release_child(&child)
            .expect("a terminal child releases");
        assert_eq!(node.reserved.get(Dimension::ActiveChildren), before);
        assert_eq!(node.used.get(Dimension::ActiveChildren), 0);
    }

    #[test]
    fn conserved_budget_rolls_up_actual_use_and_returns_the_remainder() {
        let mut node = parent();
        let mut child = node
            .spawn(DimensionVector::uniform(10), StructuralLimits::default())
            .expect("a grant inside headroom");
        child
            .consume(Dimension::CostMicroUsd, 4)
            .expect("inside the child's own limit");
        node.release_child(&child)
            .expect("a terminal child releases");
        assert_eq!(node.reserved.get(Dimension::CostMicroUsd), 0);
        assert_eq!(node.used.get(Dimension::CostMicroUsd), 4);
        assert_eq!(node.free(Dimension::CostMicroUsd), 96);
    }

    #[test]
    fn depth_is_a_structural_refusal_not_a_budget_one() {
        let structural = StructuralLimits {
            max_depth: 1,
            max_fanout: 4,
        };
        let mut root = parent();
        let mut child = root
            .spawn(DimensionVector::uniform(10), structural)
            .expect("depth one is admitted");
        let error = child
            .spawn(DimensionVector::uniform(1), structural)
            .expect_err("depth two is refused");
        assert!(
            matches!(
                error,
                BudgetError::DepthExceeded {
                    depth: 2,
                    max_depth: 1
                }
            ),
            "{error:?}"
        );
    }

    #[test]
    fn consuming_past_the_limit_is_refused_and_changes_nothing() {
        let mut node = BudgetNode::root(DimensionVector::uniform(3));
        node.consume(Dimension::ProviderCalls, 3)
            .expect("exactly the limit");
        let error = node
            .consume(Dimension::ProviderCalls, 1)
            .expect_err("one past the limit");
        assert!(matches!(error, BudgetError::Exhausted { .. }), "{error:?}");
        assert_eq!(node.used.get(Dimension::ProviderCalls), 3);
        assert!(node.invariant_i1());
    }

    #[test]
    fn a_release_that_was_never_reserved_is_a_typed_error() {
        let mut node = parent();
        let stranger = BudgetNode::root(DimensionVector::uniform(5));
        let error = node
            .release_child(&stranger)
            .expect_err("a stranger's grant was never reserved here");
        assert!(
            matches!(error, BudgetError::ReleaseUnderflow { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn the_launch_grant_names_the_recorded_defaults() {
        let grant = launch_session_grant(5_000_000);
        assert_eq!(grant.get(Dimension::ActiveChildren), 12);
        assert_eq!(grant.get(Dimension::QueuedChildren), 12);
        assert_eq!(grant.get(Dimension::TotalChildrenCreated), 12);
        assert_eq!(grant.get(Dimension::HandsCalls), 4_096);
        assert_eq!(grant.get(Dimension::CostMicroUsd), 5_000_000);
        assert_eq!(
            grant.get(Dimension::RetainedResultBytes),
            256 * 1_024 * 1_024
        );
    }

    #[test]
    fn each_dimension_declares_exactly_one_kind() {
        assert_eq!(Dimension::CostMicroUsd.kind(), Kind::Conserved);
        assert_eq!(Dimension::ActiveChildren.kind(), Kind::Concurrent);
        assert_eq!(Dimension::RetainedResultBytes.kind(), Kind::Concurrent);
    }
}
