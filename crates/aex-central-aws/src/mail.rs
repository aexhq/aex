//! The invitation notification, as a durable intent rather than a delivery.
//!
//! # Why no email vendor appears here (OD-40)
//!
//! Nothing in the accepted design picks one. `central-control-worker` is the one
//! deployable that holds a `MailSend` capability and an `ses:SendEmail`
//! permission, and its `invitation.email.deliver` duty is what actually sends.
//! An adapter here that opened its own vendor client would be a second sender,
//! in a deployable whose reviewable `PERMISSIONS` list says it cannot send at
//! all — the composition check refuses the binding, so the code would be dead
//! or the deployable would be over-privileged.
//!
//! So [`MailerPort`] is implemented as what the API can honestly promise: the
//! intent becomes **durable**, in the same store as the invitation, under the
//! outbox topic the worker's duty table already claims. `send_invitation`
//! returning `Ok` means "this will be delivered", not "this was delivered", and
//! the port's own documentation already says a lost notification costs a resend
//! and nothing else.
//!
//! The row is keyed by the outbox message id the invitation transaction
//! preassigned, and `(topic, dedupe_key)` is unique, so a retry produces one
//! message rather than a second email.

use std::fmt;
use std::sync::Arc;

use aex_control_app::ports::{ControlStore, EffectError, InvitationEmail, MailerPort, StoreError};
use aex_control_domain::{OutboxMessage, Topic};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// The notification body the worker's duty reads.
///
/// It carries **no credential**. An invitation is redeemed by proving the
/// address, not by presenting a token from the message, so there is nothing
/// here whose disclosure grants anything. `an_invitation_notification_carries_no_credential`
/// asserts the shape stays that way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InvitationNotification {
    /// The recipient address, already normalized.
    pub to: String,
    /// The organization's display name, for the body.
    pub organization_name: String,
    /// The role offered.
    pub role: String,
}

/// One notification, ready to be made durable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingNotification {
    /// The row id, which is also the dispatch idempotency key.
    pub id: Uuid,
    /// Which topic the worker claims it under.
    pub topic: Topic,
    /// The producer-side deduplication key, unique inside the topic.
    pub dedupe_key: String,
    /// The FIFO group key.
    pub group_key: String,
    /// The body.
    pub payload: serde_json::Value,
    /// When it first becomes claimable.
    pub available_at: OffsetDateTime,
}

/// Where a notification intent becomes durable.
///
/// A port rather than a direct Aurora call because the writer belongs to the
/// same store as the invitation and this crate holds no database connection:
/// a mailer that opened its own would be able to record a notification for an
/// invitation that never committed.
#[async_trait]
pub trait OutboxWriter: Send + Sync + fmt::Debug {
    /// Records one message, idempotently under `(topic, dedupe_key)`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`]. A conflict on the dedupe index is **not** an
    /// error the caller sees: the intent is already durable, which is the
    /// entire promise.
    async fn enqueue(&self, message: &PendingNotification) -> Result<(), StoreError>;
}

/// The outbox this platform actually has: the control store's own table.
///
/// The message is written through [`ControlStore`] rather than through a
/// connection of this crate's own, so the row lands in the same table, under
/// the same role and the same unique index, as every outbox row the invitation
/// transaction commits inline. A second writer would be a second place for the
/// worker's `(topic, dedupe_key)` contract to be spelled.
pub struct ControlStoreOutbox {
    store: Arc<dyn ControlStore>,
}

impl fmt::Debug for ControlStoreOutbox {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("ControlStoreOutbox").finish()
    }
}

impl ControlStoreOutbox {
    /// Builds the writer over one control store.
    #[must_use]
    pub fn new(store: Arc<dyn ControlStore>) -> Self {
        Self { store }
    }

    /// The durable row one pending notification becomes.
    ///
    /// Every dispatch-state column is the value a freshly committed row carries:
    /// no attempts, no claim, not dispatched, no error. A writer that filled any
    /// of them would be pre-deciding something only the worker may.
    #[must_use]
    pub fn row(message: &PendingNotification) -> OutboxMessage {
        OutboxMessage {
            id: message.id,
            topic: message.topic,
            dedupe_key: message.dedupe_key.clone(),
            group_key: message.group_key.clone(),
            payload: message.payload.clone(),
            attempts: 0,
            available_at: message.available_at,
            claimed_by: None,
            claimed_until: None,
            dispatched_at: None,
            last_error: None,
            created_at: message.available_at,
        }
    }
}

#[async_trait]
impl OutboxWriter for ControlStoreOutbox {
    async fn enqueue(&self, message: &PendingNotification) -> Result<(), StoreError> {
        self.store.enqueue_outbox(&Self::row(message)).await
    }
}

/// The mailer that writes an intent and sends nothing.
#[derive(Debug, Clone)]
pub struct OutboxMailer {
    writer: Arc<dyn OutboxWriter>,
}

impl OutboxMailer {
    /// Builds the mailer over one outbox.
    #[must_use]
    pub fn new(writer: Arc<dyn OutboxWriter>) -> Self {
        Self { writer }
    }

    /// The durable message one invitation notification becomes.
    ///
    /// Public so a deployable's own suite can assert the exact row it will
    /// write without reaching for a store.
    #[must_use]
    pub fn message(mail: &InvitationEmail, now: OffsetDateTime) -> PendingNotification {
        PendingNotification {
            id: mail.message_id,
            topic: Topic::InvitationEmailRequested,
            // The outbox row id the invitation transaction preassigned. Two
            // deliveries of the same invitation share it, so the unique index
            // collapses them into one message.
            dedupe_key: mail.message_id.to_string(),
            // Grouped by the message rather than the recipient: two invitations
            // to one address, for two organizations, are independent and must
            // not serialize behind each other.
            group_key: mail.message_id.to_string(),
            payload: serde_json::json!({
                "to": mail.to,
                "organizationName": mail.organization_name,
                "role": mail.role.as_str(),
            }),
            available_at: now,
        }
    }
}

/// Maps a store failure onto the effect vocabulary.
///
/// A conflict is success: the unique index caught a retry of a message that is
/// already durable, and the promise `send_invitation` makes is durability.
const fn effect_of(error: &StoreError) -> Result<(), EffectError> {
    match error {
        StoreError::Conflict { .. } => Ok(()),
        StoreError::Unavailable => Err(EffectError::Unavailable),
        StoreError::Unknown => Err(EffectError::Unknown),
        StoreError::PermissionDenied => Err(EffectError::Rejected {
            code: "mail_outbox_permission_denied",
            retryable: false,
        }),
        StoreError::NotFound => Err(EffectError::Rejected {
            code: "mail_outbox_absent",
            retryable: false,
        }),
        StoreError::Decode(_) | StoreError::Fatal(_) => Err(EffectError::Rejected {
            code: "mail_outbox_refused",
            retryable: false,
        }),
    }
}

#[async_trait]
impl MailerPort for OutboxMailer {
    async fn send_invitation(&self, mail: &InvitationEmail) -> Result<(), EffectError> {
        // The instant is the row's own `available_at`; the worker claims it as
        // soon as it is committed, so "now" is the earliest correct value and
        // this port holds no clock of its own to disagree with the store's.
        let message = Self::message(mail, OffsetDateTime::now_utc());
        match self.writer.enqueue(&message).await {
            Ok(()) => Ok(()),
            Err(error) => effect_of(&error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        InvitationNotification, OutboxMailer, OutboxWriter, PendingNotification, effect_of,
    };
    use aex_control_app::ports::{EffectError, InvitationEmail, MailerPort as _, StoreError};
    use aex_control_domain::{OrgRole, Topic};
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};
    use time::OffsetDateTime;
    use uuid::Uuid;

    #[derive(Debug, Default)]
    struct Recording {
        written: Mutex<Vec<PendingNotification>>,
        answer: Mutex<Option<StoreError>>,
    }

    #[async_trait]
    impl OutboxWriter for Recording {
        async fn enqueue(&self, message: &PendingNotification) -> Result<(), StoreError> {
            self.written
                .lock()
                .expect("the recorder is not poisoned")
                .push(message.clone());
            match self
                .answer
                .lock()
                .expect("the recorder is not poisoned")
                .clone()
            {
                Some(error) => Err(error),
                None => Ok(()),
            }
        }
    }

    fn mail() -> InvitationEmail {
        InvitationEmail {
            message_id: Uuid::from_u128(7),
            to: "person@example.test".to_owned(),
            organization_name: "Acme".to_owned(),
            role: OrgRole::Admin,
        }
    }

    fn run<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime")
            .block_on(future)
    }

    #[test]
    fn a_notification_becomes_one_durable_row_under_the_worker_s_topic() {
        let writer = Arc::new(Recording::default());
        let mailer = OutboxMailer::new(Arc::clone(&writer) as Arc<dyn OutboxWriter>);
        run(mailer.send_invitation(&mail())).expect("the intent is durable");
        let written = writer.written.lock().expect("not poisoned");
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].topic, Topic::InvitationEmailRequested);
        assert_eq!(written[0].id, Uuid::from_u128(7));
    }

    #[test]
    fn two_deliveries_of_one_invitation_share_a_dedupe_key() {
        let first = OutboxMailer::message(&mail(), OffsetDateTime::UNIX_EPOCH);
        let second =
            OutboxMailer::message(&mail(), OffsetDateTime::UNIX_EPOCH + time::Duration::HOUR);
        assert_eq!(
            first.dedupe_key, second.dedupe_key,
            "the unique index is what collapses a retry into one email"
        );
    }

    #[test]
    fn an_invitation_notification_carries_no_credential() {
        let message = OutboxMailer::message(&mail(), OffsetDateTime::UNIX_EPOCH);
        let decoded: InvitationNotification =
            serde_json::from_value(message.payload.clone()).expect("the declared shape");
        assert_eq!(decoded.to, "person@example.test");
        assert_eq!(decoded.role, "admin");
        let rendered = message.payload.to_string();
        for forbidden in ["token", "secret", "code", "verifier", "pepper"] {
            assert!(
                !rendered.contains(forbidden),
                "`{forbidden}` reached a notification body: {rendered}"
            );
        }
    }

    #[test]
    fn the_control_store_writer_lands_a_fresh_undispatched_row() {
        let notification = OutboxMailer::message(&mail(), OffsetDateTime::UNIX_EPOCH);
        let row = super::ControlStoreOutbox::row(&notification);
        assert_eq!(row.id, notification.id);
        assert_eq!(row.topic, Topic::InvitationEmailRequested);
        assert_eq!(row.dedupe_key, notification.dedupe_key);
        assert_eq!(row.group_key, notification.group_key);
        assert_eq!(row.payload, notification.payload);
        assert_eq!(row.available_at, notification.available_at);
        // Every dispatch-state column belongs to the worker. A writer that set
        // one would be deciding an attempt, a claim or a delivery it never made.
        assert_eq!(row.attempts, 0);
        assert_eq!(row.claimed_by, None);
        assert_eq!(row.claimed_until, None);
        assert_eq!(row.dispatched_at, None);
        assert_eq!(row.last_error, None);
    }

    #[test]
    fn a_dedupe_conflict_is_the_promise_already_kept() {
        let writer = Arc::new(Recording::default());
        *writer.answer.lock().expect("not poisoned") = Some(StoreError::Conflict {
            constraint: "outbox_dedupe_uk".to_owned(),
        });
        let mailer = OutboxMailer::new(Arc::clone(&writer) as Arc<dyn OutboxWriter>);
        run(mailer.send_invitation(&mail())).expect("an already durable intent is success");
    }

    #[test]
    fn an_unknown_write_stays_unknown_rather_than_becoming_a_failure() {
        assert_eq!(effect_of(&StoreError::Unknown), Err(EffectError::Unknown));
        assert_eq!(
            effect_of(&StoreError::Unavailable),
            Err(EffectError::Unavailable)
        );
    }

    #[test]
    fn a_denied_role_is_refused_without_a_retry_hint() {
        assert_eq!(
            effect_of(&StoreError::PermissionDenied),
            Err(EffectError::Rejected {
                code: "mail_outbox_permission_denied",
                retryable: false
            })
        );
    }
}
