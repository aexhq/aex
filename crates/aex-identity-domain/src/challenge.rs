//! Email sign-in challenges.
//!
//! `Issued → {Consumed, Expired}`, TTL 15 minutes. Consumption is a
//! constant-time verifier match **and then** a conditional
//! `UPDATE … WHERE consumed_at IS NULL AND expires_at > :now`. A wrong secret
//! never reaches the `UPDATE`, so an attacker who guesses cannot burn a live
//! challenge, and two valid concurrent consumers yield exactly one winner.

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::credential::PepperVersion;
use crate::email::NormalizedEmail;

/// How long a sign-in link lives.
pub const EMAIL_CHALLENGE_TTL: Duration = Duration::minutes(15);

/// Where a challenge is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChallengeState {
    /// Not used yet.
    Issued,
    /// Redeemed.
    Consumed,
    /// Past its expiry.
    Expired,
}

/// A single-use email sign-in link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailChallenge {
    /// The challenge's own id, which is the credential lookup key.
    pub id: Uuid,
    /// Which address it proves.
    pub email: NormalizedEmail,
    /// Which pepper version the verifier was computed under.
    pub pepper_version: PepperVersion,
    /// When it was minted.
    pub issued_at: OffsetDateTime,
    /// When it stops being redeemable.
    pub expires_at: OffsetDateTime,
    /// When it was redeemed.
    pub consumed_at: Option<OffsetDateTime>,
}

/// Why a challenge transition was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ChallengeTransition {
    /// The challenge was already redeemed.
    #[error("the challenge was already used")]
    AlreadyConsumed,
    /// The challenge lapsed.
    #[error("the challenge expired")]
    Expired,
}

impl EmailChallenge {
    /// Where the challenge is at `now`.
    #[must_use]
    pub fn state_at(&self, now: OffsetDateTime) -> ChallengeState {
        // Consumption is a recorded fact, not a comparison: a consumed
        // challenge is consumed at every instant, including one before the
        // recorded timestamp. A clock that moves backwards must not resurrect a
        // single-use credential.
        if self.consumed_at.is_some() {
            return ChallengeState::Consumed;
        }
        if self.expires_at <= now {
            return ChallengeState::Expired;
        }
        ChallengeState::Issued
    }

    /// Redeems the challenge.
    ///
    /// # Errors
    ///
    /// Returns [`ChallengeTransition::AlreadyConsumed`] for a repeat and
    /// [`ChallengeTransition::Expired`] past the TTL. The adapter applies the
    /// same two guards as a SQL predicate, so correctness does not depend on the
    /// application winning a race.
    pub fn consume(&self, now: OffsetDateTime) -> Result<Self, ChallengeTransition> {
        match self.state_at(now) {
            ChallengeState::Consumed => Err(ChallengeTransition::AlreadyConsumed),
            ChallengeState::Expired => Err(ChallengeTransition::Expired),
            ChallengeState::Issued => Ok(Self {
                consumed_at: Some(now),
                ..self.clone()
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ChallengeState, ChallengeTransition, EMAIL_CHALLENGE_TTL, EmailChallenge};
    use crate::credential::PepperVersion;
    use crate::email::NormalizedEmail;
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    fn challenge() -> EmailChallenge {
        EmailChallenge {
            id: Uuid::from_u128(1),
            email: NormalizedEmail::parse("a@b.test").expect("valid"),
            pepper_version: PepperVersion::new(1),
            issued_at: OffsetDateTime::UNIX_EPOCH,
            expires_at: OffsetDateTime::UNIX_EPOCH + EMAIL_CHALLENGE_TTL,
            consumed_at: None,
        }
    }

    #[test]
    fn the_ttl_is_fifteen_minutes_and_the_boundary_is_exact() {
        assert_eq!(EMAIL_CHALLENGE_TTL, Duration::minutes(15));
        let challenge = challenge();
        assert_eq!(
            challenge.state_at(challenge.expires_at - Duration::milliseconds(1)),
            ChallengeState::Issued
        );
        assert_eq!(
            challenge.state_at(challenge.expires_at),
            ChallengeState::Expired
        );
    }

    #[test]
    fn a_challenge_is_consumed_at_most_once() {
        let now = OffsetDateTime::UNIX_EPOCH + Duration::minutes(1);
        let consumed = challenge().consume(now).expect("first consumption");
        assert_eq!(consumed.consumed_at, Some(now));
        assert_eq!(
            consumed.consume(now),
            Err(ChallengeTransition::AlreadyConsumed)
        );
    }

    #[test]
    fn an_expired_challenge_is_never_redeemable() {
        let now = OffsetDateTime::UNIX_EPOCH + EMAIL_CHALLENGE_TTL;
        assert_eq!(challenge().consume(now), Err(ChallengeTransition::Expired));
    }

    #[test]
    fn a_consumed_challenge_never_returns_to_issued() {
        let now = OffsetDateTime::UNIX_EPOCH + Duration::minutes(1);
        let consumed = challenge().consume(now).expect("consumption");
        for at in [
            now,
            now + Duration::minutes(1),
            consumed.expires_at + Duration::days(1),
        ] {
            assert_eq!(consumed.state_at(at), ChallengeState::Consumed, "{at}");
        }
    }
}
