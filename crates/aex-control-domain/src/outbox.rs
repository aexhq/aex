//! The transactional outbox.
//!
//! An outbox row is committed in the same transaction as the aggregate it
//! describes, then claimed with a bounded lease, dispatched, and marked. The
//! topic list is closed by a `CHECK`, and `(topic, dedupe_key)` is unique, so a
//! retried command produces one message rather than a duplicate.
//!
//! The system this replaces had inserts into `account_outbox` and no reader at
//! all: `delivered_at` was never set and there was no index to find an
//! undispatched row by.

use time::OffsetDateTime;
use uuid::Uuid;

/// The closed topic list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Topic {
    /// A workspace's regional half must be created.
    WorkspaceProvisionRequested,
    /// A workspace's regional half must be removed.
    WorkspaceDeleteRequested,
    /// A finance account state or epoch must be projected to one workspace.
    AccountStateChanged,
    /// An invitation notification must be sent.
    InvitationEmailRequested,
    /// A workspace API key was minted and every region must learn it exists.
    ///
    /// A region cannot admit a request against a key it has never heard of, so
    /// this is committed in the same transaction as the key itself. Without it
    /// the projection could only ever record revocations, and "no row" would
    /// have to mean "not revoked" rather than "no such key".
    ApiKeyCreated,
    /// A revocation epoch advanced and every region must learn it.
    AuthorizationEpochChanged,
    /// A new assertion signing key must be projected to every region.
    AuthorizationSigningKeyPublished,
}

impl Topic {
    /// Every topic.
    pub const ALL: [Self; 7] = [
        Self::WorkspaceProvisionRequested,
        Self::WorkspaceDeleteRequested,
        Self::AccountStateChanged,
        Self::InvitationEmailRequested,
        Self::ApiKeyCreated,
        Self::AuthorizationEpochChanged,
        Self::AuthorizationSigningKeyPublished,
    ];

    /// The database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WorkspaceProvisionRequested => "workspace.provision.requested",
            Self::WorkspaceDeleteRequested => "workspace.delete.requested",
            Self::AccountStateChanged => "account.state.changed",
            Self::InvitationEmailRequested => "invitation.email.requested",
            Self::ApiKeyCreated => "api_key.created",
            Self::AuthorizationEpochChanged => "authorization.epoch.changed",
            Self::AuthorizationSigningKeyPublished => "authorization.signing_key.published",
        }
    }

    /// Resolves a database spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.as_str() == text)
    }
}

/// One outbox row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxMessage {
    /// The row's own id, which is also the dispatch idempotency key.
    pub id: Uuid,
    /// Which topic.
    pub topic: Topic,
    /// The producer-side deduplication key, unique inside the topic.
    pub dedupe_key: String,
    /// The FIFO group key. Always the organization id on the money path.
    pub group_key: String,
    /// The message body.
    pub payload: serde_json::Value,
    /// How many dispatch attempts have been made.
    pub attempts: u32,
    /// When it next becomes claimable.
    pub available_at: OffsetDateTime,
    /// Which worker holds the claim.
    pub claimed_by: Option<String>,
    /// When that claim lapses.
    pub claimed_until: Option<OffsetDateTime>,
    /// When it was successfully dispatched.
    pub dispatched_at: Option<OffsetDateTime>,
    /// A redacted rendering of the last failure.
    pub last_error: Option<String>,
    /// When it was committed.
    pub created_at: OffsetDateTime,
}

/// Why an outbox transition was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OutboxTransition {
    /// The message was already dispatched.
    #[error("the message was already dispatched")]
    AlreadyDispatched,
    /// Another worker holds an unexpired claim.
    #[error("another worker holds the claim")]
    Claimed,
    /// The attempt ceiling was reached; the message belongs in the dead letter.
    #[error("the message reached its attempt ceiling of {ceiling}")]
    AttemptCeiling {
        /// The ceiling.
        ceiling: u32,
    },
}

impl OutboxMessage {
    /// The most dispatch attempts one message may have.
    pub const MAX_ATTEMPTS: u32 = 100;

    /// Whether the message is claimable at `now`.
    #[must_use]
    pub fn is_claimable_at(&self, now: OffsetDateTime) -> bool {
        self.dispatched_at.is_none()
            && self.available_at <= now
            && self.claimed_until.is_none_or(|until| until <= now)
    }

    /// Claims the message.
    ///
    /// # Errors
    ///
    /// Returns [`OutboxTransition::AlreadyDispatched`] for a dispatched
    /// message, [`OutboxTransition::Claimed`] while another claim is live, and
    /// [`OutboxTransition::AttemptCeiling`] at the ceiling.
    pub fn claim(
        &self,
        worker: &str,
        now: OffsetDateTime,
        lease: time::Duration,
    ) -> Result<Self, OutboxTransition> {
        if self.dispatched_at.is_some() {
            return Err(OutboxTransition::AlreadyDispatched);
        }
        if !self.is_claimable_at(now) {
            return Err(OutboxTransition::Claimed);
        }
        if self.attempts >= Self::MAX_ATTEMPTS {
            return Err(OutboxTransition::AttemptCeiling {
                ceiling: Self::MAX_ATTEMPTS,
            });
        }
        Ok(Self {
            attempts: self.attempts + 1,
            claimed_by: Some(worker.to_owned()),
            claimed_until: Some(now + lease),
            ..self.clone()
        })
    }

    /// Marks the message dispatched.
    ///
    /// # Errors
    ///
    /// Returns [`OutboxTransition::AlreadyDispatched`] for a repeat.
    pub fn mark_dispatched(&self, now: OffsetDateTime) -> Result<Self, OutboxTransition> {
        if self.dispatched_at.is_some() {
            return Err(OutboxTransition::AlreadyDispatched);
        }
        Ok(Self {
            dispatched_at: Some(now),
            claimed_by: None,
            claimed_until: None,
            last_error: None,
            ..self.clone()
        })
    }

    /// Releases the claim after a failure, backing off to `available_at`.
    ///
    /// # Errors
    ///
    /// Returns [`OutboxTransition::AlreadyDispatched`] for a dispatched message.
    pub fn release(
        &self,
        available_at: OffsetDateTime,
        error: &str,
    ) -> Result<Self, OutboxTransition> {
        if self.dispatched_at.is_some() {
            return Err(OutboxTransition::AlreadyDispatched);
        }
        Ok(Self {
            claimed_by: None,
            claimed_until: None,
            available_at,
            last_error: Some(error.to_owned()),
            ..self.clone()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{OutboxMessage, OutboxTransition, Topic};
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    fn message() -> OutboxMessage {
        OutboxMessage {
            id: Uuid::from_u128(1),
            topic: Topic::WorkspaceProvisionRequested,
            dedupe_key: "wsp-1".to_owned(),
            group_key: "org-1".to_owned(),
            payload: serde_json::json!({}),
            attempts: 0,
            available_at: OffsetDateTime::UNIX_EPOCH,
            claimed_by: None,
            claimed_until: None,
            dispatched_at: None,
            last_error: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn a_claim_is_exclusive_until_it_lapses() {
        let now = OffsetDateTime::UNIX_EPOCH;
        let lease = Duration::seconds(30);
        let claimed = message().claim("a", now, lease).expect("first claim");
        assert_eq!(claimed.attempts, 1);
        assert_eq!(
            claimed.claim("b", now + Duration::seconds(10), lease),
            Err(OutboxTransition::Claimed)
        );
        assert!(
            claimed
                .claim("b", now + Duration::seconds(31), lease)
                .is_ok()
        );
    }

    #[test]
    fn a_dispatched_message_is_never_claimed_again() {
        let now = OffsetDateTime::UNIX_EPOCH;
        let done = message().mark_dispatched(now).expect("dispatch");
        assert_eq!(
            done.claim("a", now, Duration::seconds(30)),
            Err(OutboxTransition::AlreadyDispatched)
        );
        assert_eq!(
            done.mark_dispatched(now),
            Err(OutboxTransition::AlreadyDispatched)
        );
        assert_eq!(
            done.release(now, "x"),
            Err(OutboxTransition::AlreadyDispatched)
        );
        assert!(!done.is_claimable_at(now));
    }

    #[test]
    fn a_release_backs_off_and_records_a_redacted_reason() {
        let now = OffsetDateTime::UNIX_EPOCH;
        let claimed = message()
            .claim("a", now, Duration::seconds(30))
            .expect("claim");
        let released = claimed
            .release(now + Duration::seconds(60), "regional_unavailable")
            .expect("release");
        assert_eq!(released.claimed_by, None);
        assert_eq!(released.available_at, now + Duration::seconds(60));
        assert_eq!(released.last_error.as_deref(), Some("regional_unavailable"));
        assert!(!released.is_claimable_at(now));
        assert!(released.is_claimable_at(now + Duration::seconds(60)));
    }

    #[test]
    fn the_attempt_ceiling_sends_a_poison_message_to_the_dead_letter() {
        let mut message = message();
        message.attempts = OutboxMessage::MAX_ATTEMPTS;
        assert_eq!(
            message.claim("a", OffsetDateTime::UNIX_EPOCH, Duration::seconds(30)),
            Err(OutboxTransition::AttemptCeiling {
                ceiling: OutboxMessage::MAX_ATTEMPTS
            })
        );
    }

    #[test]
    fn the_topic_list_is_closed_and_round_trips() {
        for topic in Topic::ALL {
            assert_eq!(Topic::parse(topic.as_str()), Some(topic));
        }
        assert_eq!(Topic::parse("workspace.provision.done"), None);
        assert_eq!(Topic::ALL.len(), 7);
    }
}
