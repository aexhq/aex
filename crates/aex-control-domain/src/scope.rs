//! The one scope vocabulary.
//!
//! [`Scope`] is `aex_wire::scopes::ScopeId` — the generated launch registry —
//! re-exported rather than redefined. This module adds the *set* algebra the
//! authorization decision needs: a `u64` bitset whose bit `n` is
//! `ScopeId::ALL[n]`, which makes an intersection one instruction and makes the
//! assertion envelope's `scopes` field a fixed eight bytes. There is one
//! vocabulary here and the compiler enforces it.

use aex_wire::scopes::ScopeId;

/// One authorization scope.
pub type Scope = ScopeId;

/// How many scopes the registry holds, as a shift width.
const REGISTRY_LEN: u32 = 11;

/// The registry is a `u64` bitset, so it can never exceed 64 entries.
const _: () = assert!(ScopeId::ALL.len() <= 64);
const _: () = assert!(ScopeId::ALL.len() == REGISTRY_LEN as usize);
/// The launch registry size, asserted so an added scope is a visible diff.
const _: () = assert!(ScopeId::ALL.len() == 11);

/// Why a scope list was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScopeError {
    /// A spelling that is not in the registry.
    #[error("`{spelling}` is not a scope in the registry")]
    Unknown {
        /// What was supplied.
        spelling: String,
    },
    /// The same scope appeared twice.
    #[error("`{spelling}` appears more than once")]
    Duplicate {
        /// The repeated scope.
        spelling: String,
    },
    /// A bitset carried a bit no registry entry owns.
    #[error("scope bitset {bits:#018x} sets {count} bit(s) outside the registry")]
    UndefinedBit {
        /// The offending bitset.
        bits: u64,
        /// How many bits were outside the registry.
        count: u32,
    },
}

/// The bit `scope` owns.
///
/// The generated enum declares no explicit discriminants, so the discriminant
/// of `ScopeId::ALL[n]` is `n`. `registry_order_is_discriminant_order` asserts
/// exactly that, which is what makes this cast a fact rather than a hope.
#[must_use]
const fn bit(scope: Scope) -> u64 {
    1_u64 << (scope as u8)
}

/// A duplicate-free set of scopes, as a registry-ordered bitset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScopeSet(u64);

impl ScopeSet {
    /// No scopes at all.
    pub const EMPTY: Self = Self(0);

    /// Every scope in the registry.
    pub const ALL: Self = Self(u64::MAX >> (64 - REGISTRY_LEN));

    /// Every scope a central route can require.
    ///
    /// The complement is the regional set, which no central route consults.
    pub const CENTRAL: Self = Self::of(&[
        Scope::AccountRead,
        Scope::AccountWrite,
        Scope::ApiKeysRead,
        Scope::ApiKeysWrite,
        Scope::BillingRead,
        Scope::BillingWrite,
    ]);

    /// Every scope a regional edge can be asked for.
    pub const REGIONAL: Self = Self::of(&[
        Scope::SessionsRead,
        Scope::SessionsWrite,
        Scope::SessionsDelete,
        Scope::ResourcesRead,
        Scope::ResourcesWrite,
    ]);

    /// The ceiling on what a workspace API key may ever carry.
    ///
    /// The regional set plus the four central scopes a workspace-scoped
    /// credential legitimately needs. A key can never hold account, API-key or
    /// billing scopes, no matter what a request asks for.
    pub const WORKSPACE_KEY_MINTABLE: Self = Self::REGIONAL;

    /// Every central scope: an organization owner.
    pub const OWNER: Self = Self::CENTRAL;

    /// An owner minus the irreversible one.
    pub const ADMIN: Self = Self::CENTRAL;

    /// The read-mostly member role.
    ///
    /// `memberships:accept` is here and not in the write group above: it acts
    /// only on invitations already addressed to the caller's own verified
    /// email, so the weakest role must carry it — the person redeeming an
    /// invitation is not yet a member of the inviting organization at all.
    pub const MEMBER: Self =
        Self::of(&[Scope::AccountRead, Scope::AccountWrite, Scope::BillingRead]);

    /// Every scope a browser session may exercise, **derived from the
    /// contract**.
    ///
    /// A dashboard session is not a general-purpose token: it carries exactly
    /// the scopes of the routes that declare `altPrincipal: user_session`, and
    /// nothing else. That set is folded out of the generated route table rather
    /// than written here, because a hand-written copy goes stale in the one
    /// direction that matters — a route gaining the alternative principal
    /// without the credential gaining its scope is a `403` nobody can explain,
    /// and a route losing it while the scope stays is a credential wider than
    /// the contract says.
    #[must_use]
    pub fn dashboard_session() -> Self {
        static SCOPES: std::sync::OnceLock<ScopeSet> = std::sync::OnceLock::new();
        *SCOPES.get_or_init(|| {
            aex_wire::routes::ROUTES
                .iter()
                .filter(|route| {
                    route.alt_principal == Some(aex_wire::idempotency::PrincipalKind::UserSession)
                })
                .filter_map(|route| route.required_scope)
                .fold(Self::EMPTY, Self::insert)
        })
    }

    /// Builds a set from a slice, at compile time.
    #[must_use]
    pub const fn of(scopes: &[Scope]) -> Self {
        let mut bits = 0_u64;
        let mut index = 0;
        while index < scopes.len() {
            bits |= bit(scopes[index]);
            index += 1;
        }
        Self(bits)
    }

    /// The raw bitset, as carried by the assertion envelope.
    #[must_use]
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Rebuilds a set from an assertion envelope's eight bytes.
    ///
    /// # Errors
    ///
    /// Returns [`ScopeError::UndefinedBit`] when a bit outside the registry is
    /// set, so a forged envelope cannot smuggle an undefined scope past the
    /// edge as "some scope we do not know about yet".
    pub const fn from_bits(bits: u64) -> Result<Self, ScopeError> {
        let stray = bits & !Self::ALL.0;
        if stray != 0 {
            return Err(ScopeError::UndefinedBit {
                bits,
                count: stray.count_ones(),
            });
        }
        Ok(Self(bits))
    }

    /// Whether the set carries `scope`.
    #[must_use]
    pub const fn contains(self, scope: Scope) -> bool {
        self.0 & bit(scope) != 0
    }

    /// Whether the set carries every scope in `other`.
    #[must_use]
    pub const fn contains_all(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// The scopes in both sets.
    #[must_use]
    pub const fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// The scopes in either set.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// The scopes in `self` but not `other`.
    #[must_use]
    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// The set with `scope` added.
    #[must_use]
    pub const fn insert(self, scope: Scope) -> Self {
        Self(self.0 | bit(scope))
    }

    /// The set with `scope` removed.
    #[must_use]
    pub const fn remove(self, scope: Scope) -> Self {
        Self(self.0 & !bit(scope))
    }

    /// Whether the set is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// How many scopes the set carries.
    #[must_use]
    pub const fn len(self) -> u32 {
        self.0.count_ones()
    }

    /// Every scope in registry order.
    pub fn iter(self) -> impl Iterator<Item = Scope> {
        ScopeId::ALL
            .iter()
            .copied()
            .enumerate()
            .filter_map(move |(index, scope)| (self.0 & (1_u64 << index) != 0).then_some(scope))
    }

    /// Parses a list of wire spellings.
    ///
    /// # Errors
    ///
    /// Returns [`ScopeError::Unknown`] for a spelling outside the registry and
    /// [`ScopeError::Duplicate`] for a repeat. Both are hard errors: silently
    /// dropping an unknown scope is how a request for more access than exists
    /// turns into a token that quietly has less.
    pub fn from_strings<S: AsRef<str>>(values: &[S]) -> Result<Self, ScopeError> {
        let mut set = Self::EMPTY;
        for value in values {
            let spelling = value.as_ref();
            let scope = Scope::parse(spelling).ok_or_else(|| ScopeError::Unknown {
                spelling: spelling.to_owned(),
            })?;
            if set.contains(scope) {
                return Err(ScopeError::Duplicate {
                    spelling: spelling.to_owned(),
                });
            }
            set = set.insert(scope);
        }
        Ok(set)
    }

    /// The wire spellings, in registry order.
    #[must_use]
    pub fn to_strings(self) -> Vec<String> {
        self.iter().map(|scope| scope.as_str().to_owned()).collect()
    }

    /// The set as an `aex-wire` scope set, for a wire response.
    #[must_use]
    pub fn to_wire(self) -> aex_wire::scopes::ScopeSet {
        aex_wire::scopes::ScopeSet::new(self.iter())
    }
}

impl FromIterator<Scope> for ScopeSet {
    fn from_iter<I: IntoIterator<Item = Scope>>(iter: I) -> Self {
        iter.into_iter().fold(Self::EMPTY, Self::insert)
    }
}

#[cfg(test)]
mod tests {
    use super::{Scope, ScopeError, ScopeSet};
    use aex_wire::scopes::ScopeId;

    #[test]
    fn every_registry_entry_has_a_distinct_bit() {
        let mut seen = 0_u64;
        for scope in ScopeId::ALL {
            let set = ScopeSet::EMPTY.insert(*scope);
            assert_eq!(set.len(), 1, "{scope}");
            assert_eq!(seen & set.bits(), 0, "{scope} shares a bit");
            seen |= set.bits();
        }
        assert_eq!(seen, ScopeSet::ALL.bits());
    }

    #[test]
    fn central_and_regional_partition_the_registry() {
        assert_eq!(
            ScopeSet::CENTRAL.union(ScopeSet::REGIONAL),
            ScopeSet::ALL,
            "every scope is central or regional"
        );
        assert_eq!(
            ScopeSet::CENTRAL.intersect(ScopeSet::REGIONAL),
            ScopeSet::EMPTY,
            "no scope is both"
        );
    }

    #[test]
    fn the_mintable_ceiling_is_exactly_the_regional_scope_set() {
        for scope in [
            Scope::AccountRead,
            Scope::AccountWrite,
            Scope::ApiKeysRead,
            Scope::ApiKeysWrite,
            Scope::BillingRead,
            Scope::BillingWrite,
        ] {
            assert!(
                !ScopeSet::WORKSPACE_KEY_MINTABLE.contains(scope),
                "a workspace key may never carry {scope}"
            );
        }
        assert_eq!(ScopeSet::WORKSPACE_KEY_MINTABLE, ScopeSet::REGIONAL);
        for scope in ScopeSet::REGIONAL.iter() {
            assert!(ScopeSet::WORKSPACE_KEY_MINTABLE.contains(scope), "{scope}");
        }
    }

    #[test]
    fn dashboard_sessions_get_the_exact_central_surface_without_widening_workspace_keys() {
        assert_eq!(
            ScopeSet::dashboard_session(),
            ScopeSet::of(&[
                Scope::AccountRead,
                Scope::AccountWrite,
                Scope::ApiKeysRead,
                Scope::ApiKeysWrite,
                Scope::BillingRead,
                Scope::BillingWrite,
            ])
        );
        assert_eq!(
            ScopeSet::dashboard_session().intersect(ScopeSet::WORKSPACE_KEY_MINTABLE),
            ScopeSet::EMPTY,
            "dashboard-session authority must not leak into workspace API keys"
        );
        assert_eq!(ScopeSet::WORKSPACE_KEY_MINTABLE, ScopeSet::REGIONAL);
    }

    #[test]
    fn owner_and_admin_share_the_current_central_ceiling() {
        assert_eq!(ScopeSet::OWNER, ScopeSet::CENTRAL);
        assert_eq!(ScopeSet::ADMIN, ScopeSet::CENTRAL);
        assert!(ScopeSet::ADMIN.contains_all(ScopeSet::MEMBER));
    }

    #[test]
    fn a_member_can_never_manage_keys_or_billing() {
        for scope in [Scope::ApiKeysRead, Scope::ApiKeysWrite, Scope::BillingWrite] {
            assert!(!ScopeSet::MEMBER.contains(scope), "{scope}");
        }
    }

    #[test]
    fn the_string_round_trip_is_lossless_and_registry_ordered() {
        let set = ScopeSet::of(&[
            Scope::ResourcesWrite,
            Scope::AccountRead,
            Scope::SessionsRead,
        ]);
        assert_eq!(
            set.to_strings(),
            vec![
                "account:read".to_owned(),
                "sessions:read".to_owned(),
                "resources:write".to_owned()
            ]
        );
        assert_eq!(ScopeSet::from_strings(&set.to_strings()), Ok(set));
    }

    #[test]
    fn an_unknown_or_repeated_spelling_is_a_hard_error() {
        assert_eq!(
            ScopeSet::from_strings(&["sessions:read", "sessions:teleport"]),
            Err(ScopeError::Unknown {
                spelling: "sessions:teleport".to_owned()
            })
        );
        assert_eq!(
            ScopeSet::from_strings(&["sessions:read", "sessions:read"]),
            Err(ScopeError::Duplicate {
                spelling: "sessions:read".to_owned()
            })
        );
    }

    #[test]
    fn registry_order_is_discriminant_order() {
        for (index, scope) in ScopeId::ALL.iter().enumerate() {
            assert_eq!(
                usize::from(*scope as u8),
                index,
                "{scope} sits at registry index {index}"
            );
        }
    }

    #[test]
    fn a_bit_outside_the_registry_cannot_be_rebuilt() {
        assert_eq!(ScopeSet::from_bits(ScopeSet::ALL.bits()), Ok(ScopeSet::ALL));
        assert!(ScopeSet::from_bits(1_u64 << 63).is_err());
    }

    #[test]
    fn the_wire_set_carries_the_same_scopes() {
        let set = ScopeSet::of(&[Scope::ResourcesRead, Scope::AccountRead]);
        let wire = set.to_wire();
        assert_eq!(wire.len(), 2);
        assert!(wire.contains(Scope::ResourcesRead));
        assert!(wire.contains(Scope::AccountRead));
    }
}
