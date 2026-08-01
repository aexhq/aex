//! Reservation and closure state machines.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::money::{Microusd, MoneyError};

/// Reservation lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReservationState {
    /// Accepting settlements.
    Open,
    /// Closure declared; waiting for all facts.
    Closing,
    /// Fully settled with no release due.
    Settled,
    /// Unused credit released.
    Released,
    /// Voided before use.
    Voided,
}

/// Declared category completion, including explicit-zero facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Closure {
    declared: BTreeMap<String, bool>,
    satisfied: BTreeMap<String, bool>,
}

impl Closure {
    /// Creates an unsatisfied closure declaration.
    #[must_use]
    pub const fn new(declared: BTreeMap<String, bool>) -> Self {
        Self {
            declared,
            satisfied: BTreeMap::new(),
        }
    }

    /// Marks exactly one declaration satisfied.
    ///
    /// # Errors
    /// Rejects unknown categories and a mismatched explicit-zero assertion.
    pub fn satisfy(
        mut self,
        category: &str,
        explicit_zero: bool,
    ) -> Result<Self, ReservationError> {
        match self.declared.get(category) {
            Some(expected) if *expected == explicit_zero => {
                self.satisfied.insert(category.to_owned(), explicit_zero);
                Ok(self)
            }
            Some(_) => Err(ReservationError::ExplicitZeroMismatch),
            None => Err(ReservationError::UnknownDeclaration),
        }
    }

    /// Whether every declaration is present and byte-for-byte equal.
    #[must_use]
    pub fn is_satisfied(&self) -> bool {
        self.declared == self.satisfied
    }
}

/// Pure reservation aggregate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    state: ReservationState,
    reserved: Microusd,
    settled: Microusd,
    released: Microusd,
}

impl Reservation {
    /// Opens a reservation.
    #[must_use]
    pub const fn open(reserved: Microusd) -> Self {
        Self {
            state: ReservationState::Open,
            reserved,
            settled: Microusd::ZERO,
            released: Microusd::ZERO,
        }
    }

    /// Applies an additional settled amount.
    ///
    /// # Errors
    /// Rejects closed reservations or settlement beyond the reservation.
    pub fn apply_settlement(&self, amount: Microusd) -> Result<Self, ReservationError> {
        if !matches!(
            self.state,
            ReservationState::Open | ReservationState::Closing
        ) {
            return Err(ReservationError::InvalidState);
        }
        let settled = self.settled.checked_add(amount)?;
        if settled.get() > self.reserved.get() {
            return Err(ReservationError::OverSettlement);
        }
        let mut next = self.clone();
        next.settled = settled;
        Ok(next)
    }

    /// Starts closure.
    ///
    /// # Errors
    /// Only an open reservation can close.
    pub fn begin_close(&self) -> Result<Self, ReservationError> {
        if self.state != ReservationState::Open {
            return Err(ReservationError::InvalidState);
        }
        let mut next = self.clone();
        next.state = ReservationState::Closing;
        Ok(next)
    }

    /// Releases the exact unused balance after closure.
    ///
    /// # Errors
    /// Requires `Closing` and every declaration, including explicit zeros.
    pub fn apply_release(&self, closure: &Closure) -> Result<(Self, Microusd), ReservationError> {
        if self.state != ReservationState::Closing {
            return Err(ReservationError::InvalidState);
        }
        if !closure.is_satisfied() {
            return Err(ReservationError::ClosureUnsatisfied);
        }
        let release = self.reserved.checked_sub(self.settled)?;
        let mut next = self.clone();
        next.released = release;
        next.state = if release == Microusd::ZERO {
            ReservationState::Settled
        } else {
            ReservationState::Released
        };
        Ok((next, release))
    }

    /// State.
    #[must_use]
    pub const fn state(&self) -> ReservationState {
        self.state
    }
}

/// Reservation transition failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReservationError {
    /// Transition is not legal from the current state.
    #[error("reservation transition is invalid from this state")]
    InvalidState,
    /// Settled amount exceeds reserved amount.
    #[error("settlement exceeds the reservation")]
    OverSettlement,
    /// Not all declared facts have arrived.
    #[error("reservation closure is not satisfied")]
    ClosureUnsatisfied,
    /// A category was not declared.
    #[error("closure category was not declared")]
    UnknownDeclaration,
    /// Explicit-zero marker disagreed with the declaration.
    #[error("closure explicit-zero marker disagrees with declaration")]
    ExplicitZeroMismatch,
    /// Checked money arithmetic failed.
    #[error(transparent)]
    Money(#[from] MoneyError),
}
