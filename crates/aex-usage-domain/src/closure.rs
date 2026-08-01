//! The run / operation closure vector.
//!
//! A terminal barrier writes one immutable vector declaring, per applicable
//! public category, exactly which facts close the work. Finance evaluates
//! satisfaction and releases the escrow reservation; this crate stores the
//! vector, publishes it and proves that a fact arriving after release is
//! refused.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::identity::{FactId, SegmentOrdinal};
use crate::meter::PublicCategory;
use crate::wire_pending::{OperationId, RegionId, ReservationId, RunId, WorkspaceId};

/// What a closure vector closes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClosureAuthority {
    /// One customer run.
    Run {
        /// The run being closed.
        id: RunId,
    },
    /// One durable operation.
    Operation {
        /// The operation being closed.
        id: OperationId,
    },
}

impl ClosureAuthority {
    /// The stable kind identifier written to a row.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Run { .. } => "run",
            Self::Operation { .. } => "operation",
        }
    }

    /// The identifier of the thing being closed.
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::Run { id } => id.as_str(),
            Self::Operation { id } => id.as_str(),
        }
    }
}

impl fmt::Display for ClosureAuthority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind(), self.id())
    }
}

/// One category's closing declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosureDeclaration {
    /// Which public category this declares.
    pub category: PublicCategory,
    /// The fact that closes it.
    pub fact: FactId,
    /// Which segment of the authority that fact occupies.
    pub segment_ordinal: SegmentOrdinal,
    /// Whether the declared fact is an explicit zero.
    pub explicit_zero: bool,
}

/// Why a closure vector was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClosureError {
    /// An applicable category had no declaration.
    #[error("applicable category `{category}` has no closing declaration")]
    Undeclared {
        /// The category with no declaration.
        category: &'static str,
    },
    /// A category was declared without being applicable.
    #[error("category `{category}` is declared but not applicable")]
    NotApplicable {
        /// The category that was declared.
        category: &'static str,
    },
    /// The same `(category, ordinal)` pair appeared twice.
    #[error("category `{category}` declares segment ordinal {ordinal} twice")]
    RepeatedOrdinal {
        /// The category that repeated.
        category: &'static str,
        /// The ordinal that repeated.
        ordinal: u64,
    },
    /// A declaration marked `explicit_zero` named a non-zero fact.
    #[error("`{fact}` is declared an explicit zero but its measurement is non-zero")]
    ZeroMismatch {
        /// The fact that was mis-declared.
        fact: String,
    },
    /// A declaration named a fact that is not in the vector's own workspace and
    /// region.
    #[error("`{fact}` does not resolve inside workspace `{workspace}` region `{region}`")]
    ForeignFact {
        /// The fact that could not be resolved.
        fact: String,
        /// The workspace the vector belongs to.
        workspace: String,
        /// The region the vector belongs to.
        region: String,
    },
    /// A fact arrived for an authority whose reservation was already released.
    #[error("`{fact}` arrived after reservation `{reservation}` was released")]
    FactAfterRelease {
        /// The fact that arrived too late.
        fact: String,
        /// The reservation that had already been released.
        reservation: String,
    },
    /// An applicable category set was empty.
    #[error("a closure vector must declare at least one applicable category")]
    NothingApplicable,
}

/// One immutable statement that a run or operation is closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosureVector {
    /// What is being closed.
    pub authority: ClosureAuthority,
    /// The workspace the closed work belongs to.
    pub workspace: WorkspaceId,
    /// The region the closed work ran in.
    pub region: RegionId,
    /// The escrow reservation the work drew down.
    pub reservation: ReservationId,
    /// Which public categories this work could produce facts for. Explicit;
    /// never inferred from the declarations themselves.
    pub applicable: BTreeSet<PublicCategory>,
    /// One declaration per closing fact.
    pub declarations: Vec<ClosureDeclaration>,
    /// Whether finance has released the reservation.
    pub released: bool,
}

/// What a resolver knows about a declared fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeclaredFactFacts {
    /// Whether the fact resolves inside the vector's workspace and region.
    pub local: bool,
    /// Whether the fact's measurement is exactly zero.
    pub zero: bool,
}

impl ClosureVector {
    /// Validates the vector's own shape, without resolving any fact.
    ///
    /// # Errors
    ///
    /// Returns [`ClosureError`] when the applicable set is empty, when an
    /// applicable category has no declaration, when a declaration names a
    /// non-applicable category, or when a `(category, ordinal)` pair repeats.
    pub fn validate(&self) -> Result<(), ClosureError> {
        if self.applicable.is_empty() {
            return Err(ClosureError::NothingApplicable);
        }
        let mut seen: BTreeSet<(PublicCategory, u64)> = BTreeSet::new();
        let mut declared: BTreeMap<PublicCategory, u32> = BTreeMap::new();
        for declaration in &self.declarations {
            if !self.applicable.contains(&declaration.category) {
                return Err(ClosureError::NotApplicable {
                    category: declaration.category.id(),
                });
            }
            if !seen.insert((declaration.category, declaration.segment_ordinal.get())) {
                return Err(ClosureError::RepeatedOrdinal {
                    category: declaration.category.id(),
                    ordinal: declaration.segment_ordinal.get(),
                });
            }
            *declared.entry(declaration.category).or_default() += 1;
        }
        for category in &self.applicable {
            if !declared.contains_key(category) {
                return Err(ClosureError::Undeclared {
                    category: category.id(),
                });
            }
        }
        Ok(())
    }

    /// Validates the vector against a resolver that knows each declared fact.
    ///
    /// # Errors
    ///
    /// Returns everything [`ClosureVector::validate`] returns, plus
    /// [`ClosureError::ForeignFact`] when a declared fact does not resolve
    /// locally and [`ClosureError::ZeroMismatch`] when an `explicit_zero`
    /// declaration names a non-zero measurement.
    pub fn validate_against<R>(&self, resolve: R) -> Result<(), ClosureError>
    where
        R: Fn(&FactId) -> Option<DeclaredFactFacts>,
    {
        self.validate()?;
        for declaration in &self.declarations {
            let facts = resolve(&declaration.fact).ok_or_else(|| ClosureError::ForeignFact {
                fact: declaration.fact.to_string(),
                workspace: self.workspace.to_string(),
                region: self.region.to_string(),
            })?;
            if !facts.local {
                return Err(ClosureError::ForeignFact {
                    fact: declaration.fact.to_string(),
                    workspace: self.workspace.to_string(),
                    region: self.region.to_string(),
                });
            }
            if declaration.explicit_zero && !facts.zero {
                return Err(ClosureError::ZeroMismatch {
                    fact: declaration.fact.to_string(),
                });
            }
        }
        Ok(())
    }

    /// The closure fence: a fact may not arrive after the reservation is
    /// released.
    ///
    /// # Errors
    ///
    /// Returns [`ClosureError::FactAfterRelease`] once `released` is set.
    pub fn admit_fact(&self, fact: &FactId) -> Result<(), ClosureError> {
        if self.released {
            return Err(ClosureError::FactAfterRelease {
                fact: fact.to_string(),
                reservation: self.reservation.to_string(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClosureAuthority, ClosureDeclaration, ClosureError, ClosureVector, DeclaredFactFacts,
    };
    use crate::identity::{AuthorityId, AuthorityKey, AuthorityKind, FactId, SegmentOrdinal};
    use crate::meter::{Category, PublicCategory};
    use crate::wire_pending::{RegionId, ReservationId, RunId, WorkspaceId};
    use std::collections::BTreeSet;

    fn fact(ordinal: u64) -> FactId {
        AuthorityKey {
            region: RegionId::parse("eu-west-1").expect("region"),
            category: Category::Compute,
            kind: AuthorityKind::Run,
            authority_id: AuthorityId::parse("run-1").expect("id"),
            segment_ordinal: SegmentOrdinal::new(ordinal),
        }
        .fact_id()
    }

    fn vector(
        applicable: &[PublicCategory],
        declarations: Vec<ClosureDeclaration>,
    ) -> ClosureVector {
        ClosureVector {
            authority: ClosureAuthority::Run {
                id: RunId::parse("run-1").expect("run"),
            },
            workspace: WorkspaceId::parse("ws-1").expect("workspace"),
            region: RegionId::parse("eu-west-1").expect("region"),
            reservation: ReservationId::parse("res-1").expect("reservation"),
            applicable: applicable.iter().copied().collect::<BTreeSet<_>>(),
            declarations,
            released: false,
        }
    }

    fn declaration(
        category: PublicCategory,
        ordinal: u64,
        explicit_zero: bool,
    ) -> ClosureDeclaration {
        ClosureDeclaration {
            category,
            fact: fact(ordinal),
            segment_ordinal: SegmentOrdinal::new(ordinal),
            explicit_zero,
        }
    }

    #[test]
    fn every_applicable_category_needs_a_declaration() {
        let vector = vector(
            &[PublicCategory::Compute, PublicCategory::Memory],
            vec![declaration(PublicCategory::Compute, 0, false)],
        );
        assert!(matches!(
            vector.validate(),
            Err(ClosureError::Undeclared {
                category: "memory"
            })
        ));
    }

    #[test]
    fn a_declaration_outside_the_applicable_set_is_refused() {
        let vector = vector(
            &[PublicCategory::Compute],
            vec![
                declaration(PublicCategory::Compute, 0, false),
                declaration(PublicCategory::Storage, 1, false),
            ],
        );
        assert!(matches!(
            vector.validate(),
            Err(ClosureError::NotApplicable {
                category: "storage"
            })
        ));
    }

    #[test]
    fn a_repeated_category_ordinal_pair_is_refused() {
        let vector = vector(
            &[PublicCategory::Compute],
            vec![
                declaration(PublicCategory::Compute, 0, false),
                declaration(PublicCategory::Compute, 0, false),
            ],
        );
        assert!(matches!(
            vector.validate(),
            Err(ClosureError::RepeatedOrdinal { ordinal: 0, .. })
        ));
    }

    #[test]
    fn an_explicit_zero_must_name_a_zero_measurement() {
        let vector = vector(
            &[PublicCategory::Compute],
            vec![declaration(PublicCategory::Compute, 0, true)],
        );
        let error = vector
            .validate_against(|_| {
                Some(DeclaredFactFacts {
                    local: true,
                    zero: false,
                })
            })
            .expect_err("a non-zero fact cannot be declared an explicit zero");
        assert!(matches!(error, ClosureError::ZeroMismatch { .. }));
        assert!(
            vector
                .validate_against(|_| Some(DeclaredFactFacts {
                    local: true,
                    zero: true
                }))
                .is_ok()
        );
    }

    #[test]
    fn a_foreign_or_unresolvable_fact_is_refused() {
        let vector = vector(
            &[PublicCategory::Compute],
            vec![declaration(PublicCategory::Compute, 0, false)],
        );
        assert!(matches!(
            vector.validate_against(|_| None),
            Err(ClosureError::ForeignFact { .. })
        ));
        assert!(matches!(
            vector.validate_against(|_| Some(DeclaredFactFacts {
                local: false,
                zero: false
            })),
            Err(ClosureError::ForeignFact { .. })
        ));
    }

    #[test]
    fn a_fact_after_release_is_refused_by_the_closure_fence() {
        let mut vector = vector(
            &[PublicCategory::Compute],
            vec![declaration(PublicCategory::Compute, 0, false)],
        );
        assert!(vector.admit_fact(&fact(1)).is_ok());
        vector.released = true;
        assert!(matches!(
            vector.admit_fact(&fact(1)),
            Err(ClosureError::FactAfterRelease { .. })
        ));
    }

    #[test]
    fn an_empty_applicable_set_is_refused() {
        assert_eq!(
            vector(&[], Vec::new()).validate(),
            Err(ClosureError::NothingApplicable)
        );
    }
}
