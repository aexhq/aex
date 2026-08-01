//! Corrections and correction chains.
//!
//! A fact is never mutated. A correction is another fact that names its target,
//! and the head of a target's correction chain advances only by compare-and-set
//! on the head the corrector believed it was extending. A `Void` is terminal.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::identity::FactId;
use crate::meter::UnknownMeter;
use crate::wire_pending::{ActorRef, CaseId};

/// The longest correction chain a single target may carry.
///
/// `usage.correction_chain_max`. A deeper chain is a runaway reconciler, not a
/// legitimate restatement history.
pub const CORRECTION_CHAIN_MAX: u32 = 16;

/// Why a fact is being corrected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorrectionReason {
    /// The measured quantity was wrong.
    WrongQuantity,
    /// The fact was attributed to the wrong session, agent, run or operation.
    WrongAttribution,
    /// The fact was admitted into the wrong authority category.
    WrongCategory,
    /// The service context was wrong.
    WrongContext,
    /// The same physical measurement was recorded twice.
    DuplicateMeasurement,
    /// The provider revised the receipt the fact was derived from.
    ProviderReceiptRevised,
}

impl CorrectionReason {
    /// Every reason, in a stable order.
    pub const ALL: [Self; 6] = [
        Self::WrongQuantity,
        Self::WrongAttribution,
        Self::WrongCategory,
        Self::WrongContext,
        Self::DuplicateMeasurement,
        Self::ProviderReceiptRevised,
    ];

    /// The stable identifier written to a row.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::WrongQuantity => "wrong_quantity",
            Self::WrongAttribution => "wrong_attribution",
            Self::WrongCategory => "wrong_category",
            Self::WrongContext => "wrong_context",
            Self::DuplicateMeasurement => "duplicate_measurement",
            Self::ProviderReceiptRevised => "provider_receipt_revised",
        }
    }
}

impl fmt::Display for CorrectionReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl FromStr for CorrectionReason {
    type Err = UnknownMeter;

    fn from_str(value: &str) -> Result<Self, UnknownMeter> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.id() == value)
            .ok_or_else(|| UnknownMeter {
                kind: "correction reason",
                value: value.to_owned(),
            })
    }
}

/// What one correction fact claims.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Correction {
    /// The case that authorised the correction.
    pub case_id: CaseId,
    /// The originally admitted fact being corrected.
    pub target: FactId,
    /// The head this correction believed it was extending; `None` for the first.
    pub prior_head: Option<FactId>,
    /// Why the target is being corrected.
    pub reason: CorrectionReason,
    /// Who raised it.
    pub actor: ActorRef,
}

/// Why a correction was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CorrectionError {
    /// The correction named a different target than the head it extends.
    #[error("correction targets `{target}` but the head tracks `{head_target}`")]
    TargetMismatch {
        /// The target the correction named.
        target: String,
        /// The target the head tracks.
        head_target: String,
    },
    /// The prior head did not match the current head.
    #[error("correction expected head {expected:?} but the current head is {actual:?}")]
    HeadConflict {
        /// The head the correction claimed to extend.
        expected: Option<String>,
        /// The head that is actually current.
        actual: Option<String>,
    },
    /// The target had already been voided.
    #[error("`{target}` is voided; a void is terminal for its target")]
    TargetVoided {
        /// The target that was already withdrawn.
        target: String,
    },
    /// The chain would exceed [`CORRECTION_CHAIN_MAX`].
    #[error("correction chain for `{target}` would reach depth {depth}, over the {CORRECTION_CHAIN_MAX} ceiling")]
    ChainTooDeep {
        /// The target whose chain is full.
        target: String,
        /// The depth that was refused.
        depth: u32,
    },
    /// A correction named itself.
    #[error("`{fact}` cannot correct itself")]
    SelfReference {
        /// The identifier that appeared on both sides.
        fact: String,
    },
    /// A correction was admitted against a target that does not exist.
    #[error("`{target}` has not been admitted; a correction extends an accepted fact")]
    TargetUnknown {
        /// The target that could not be resolved.
        target: String,
    },
}

/// The head of one target's correction chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrectionHead {
    /// The originally admitted fact this chain corrects.
    pub target: FactId,
    /// The most recent correction fact, or the target itself when untouched.
    pub head: FactId,
    /// The case that produced the current head.
    pub case_id: CaseId,
    /// How many corrections have been applied.
    pub depth: u32,
    /// Whether the target has been withdrawn.
    pub voided: bool,
}

impl CorrectionHead {
    /// The head of an untouched target.
    #[must_use]
    pub fn untouched(target: FactId, case_id: CaseId) -> Self {
        Self {
            head: target.clone(),
            target,
            case_id,
            depth: 0,
            voided: false,
        }
    }

    /// Advances the chain by one correction, applied by `applied`.
    ///
    /// `prior_head` must equal the current head — `None` when nothing has
    /// corrected the target yet — the target must match, the target must not
    /// already be voided, and the chain must stay under
    /// [`CORRECTION_CHAIN_MAX`].
    ///
    /// # Errors
    ///
    /// Returns [`CorrectionError`] for every one of those refusals.
    pub fn advance(
        &self,
        correction: &Correction,
        applied: &FactId,
        voids: bool,
    ) -> Result<Self, CorrectionError> {
        if correction.target != self.target {
            return Err(CorrectionError::TargetMismatch {
                target: correction.target.to_string(),
                head_target: self.target.to_string(),
            });
        }
        if applied == &correction.target {
            return Err(CorrectionError::SelfReference {
                fact: applied.to_string(),
            });
        }
        if self.voided {
            return Err(CorrectionError::TargetVoided {
                target: self.target.to_string(),
            });
        }
        let expected = if self.depth == 0 {
            None
        } else {
            Some(self.head.clone())
        };
        if correction.prior_head != expected {
            return Err(CorrectionError::HeadConflict {
                expected: correction.prior_head.as_ref().map(FactId::to_string),
                actual: expected.as_ref().map(FactId::to_string),
            });
        }
        let depth = self.depth + 1;
        if depth > CORRECTION_CHAIN_MAX {
            return Err(CorrectionError::ChainTooDeep {
                target: self.target.to_string(),
                depth,
            });
        }
        Ok(Self {
            target: self.target.clone(),
            head: applied.clone(),
            case_id: correction.case_id.clone(),
            depth,
            voided: voids,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CORRECTION_CHAIN_MAX, Correction, CorrectionError, CorrectionHead, CorrectionReason,
    };
    use crate::identity::{AuthorityId, AuthorityKey, AuthorityKind, FactId, SegmentOrdinal};
    use crate::meter::Category;
    use crate::wire_pending::{ActorRef, CaseId, RegionId, ServiceId};

    fn fact(ordinal: u64) -> FactId {
        AuthorityKey {
            region: RegionId::parse("eu-west-1").expect("region"),
            category: Category::Compute,
            kind: AuthorityKind::Activation,
            authority_id: AuthorityId::parse("act-1").expect("id"),
            segment_ordinal: SegmentOrdinal::new(ordinal),
        }
        .fact_id()
    }

    fn case(number: u32) -> CaseId {
        CaseId::parse(&format!("case-{number}")).expect("case")
    }

    fn actor() -> ActorRef {
        ActorRef::Reconciler {
            service: ServiceId::parse("finance-reconcile").expect("service"),
        }
    }

    fn correction(target: &FactId, prior_head: Option<FactId>, number: u32) -> Correction {
        Correction {
            case_id: case(number),
            target: target.clone(),
            prior_head,
            reason: CorrectionReason::WrongQuantity,
            actor: actor(),
        }
    }

    #[test]
    fn the_chain_is_linear_and_the_prior_head_is_a_compare_and_set() {
        let target = fact(0);
        let head = CorrectionHead::untouched(target.clone(), case(0));
        let first = head
            .advance(&correction(&target, None, 1), &fact(1), false)
            .expect("first correction");
        assert_eq!(first.depth, 1);
        assert_eq!(first.head, fact(1));

        let stale = first.advance(&correction(&target, None, 2), &fact(2), false);
        assert!(matches!(stale, Err(CorrectionError::HeadConflict { .. })));

        let second = first
            .advance(&correction(&target, Some(fact(1)), 2), &fact(2), false)
            .expect("second correction");
        assert_eq!(second.depth, 2);
        assert_eq!(second.head, fact(2));
    }

    #[test]
    fn a_void_is_terminal_for_its_target() {
        let target = fact(0);
        let head = CorrectionHead::untouched(target.clone(), case(0));
        let voided = head
            .advance(&correction(&target, None, 1), &fact(1), true)
            .expect("void");
        assert!(voided.voided);
        let after = voided.advance(&correction(&target, Some(fact(1)), 2), &fact(2), false);
        assert!(matches!(after, Err(CorrectionError::TargetVoided { .. })));
    }

    #[test]
    fn a_correction_may_not_name_itself() {
        let target = fact(0);
        let head = CorrectionHead::untouched(target.clone(), case(0));
        let error = head
            .advance(&correction(&target, None, 1), &target, false)
            .expect_err("self reference is refused");
        assert!(matches!(error, CorrectionError::SelfReference { .. }));
    }

    #[test]
    fn a_correction_must_name_the_chain_it_extends() {
        let head = CorrectionHead::untouched(fact(0), case(0));
        let error = head
            .advance(&correction(&fact(9), None, 1), &fact(1), false)
            .expect_err("a foreign target is refused");
        assert!(matches!(error, CorrectionError::TargetMismatch { .. }));
    }

    #[test]
    fn the_chain_depth_is_bounded() {
        let target = fact(0);
        let mut head = CorrectionHead::untouched(target.clone(), case(0));
        for step in 1..=CORRECTION_CHAIN_MAX {
            let prior = if step == 1 {
                None
            } else {
                Some(fact(u64::from(step - 1)))
            };
            head = head
                .advance(
                    &correction(&target, prior, step),
                    &fact(u64::from(step)),
                    false,
                )
                .expect("within the ceiling");
        }
        assert_eq!(head.depth, CORRECTION_CHAIN_MAX);
        let error = head
            .advance(
                &correction(&target, Some(fact(u64::from(CORRECTION_CHAIN_MAX))), 99),
                &fact(u64::from(CORRECTION_CHAIN_MAX) + 1),
                false,
            )
            .expect_err("the ceiling holds");
        assert!(matches!(error, CorrectionError::ChainTooDeep { .. }));
    }
}
