//! The queued child's mailbox.
//!
//! `send_message` to a child that has not started yet has to go somewhere durable, because
//! the child does not exist as a running thing to deliver to. It appends to a mailbox, and
//! the child drains that mailbox in order during its first activation, **before** its first
//! model call. Delivering afterwards would mean the child answered a prompt it had not been
//! given.

use aex_brain_domain::ids::{ContentHash, Timestamp};
use aex_brain_domain::journal::{JournalRecord, MessageOrigin};
use aex_brain_domain::wire_pending::ContentBlockRef;

/// One durable mailbox entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxEntry {
    /// Its position in the mailbox. Delivery is in this order and no other.
    pub seq: u64,
    /// The content the parent sent.
    pub content: Vec<ContentBlockRef>,
    /// The caller's idempotency key.
    ///
    /// Duplicates collapse on this rather than on content: two genuinely identical messages
    /// sent deliberately are two messages, and only the caller can say which is which.
    pub idempotency_key: String,
    /// When it was appended.
    pub sent_at: Timestamp,
}

/// Turns a mailbox into the records the child's first activation appends.
///
/// Duplicates collapse by idempotency key, keeping the **first** occurrence, so the order a
/// parent sent in is the order the child sees. Keeping the last would let a retry reorder
/// messages relative to the ones sent between the original and the retry.
#[must_use]
pub fn drain(entries: &[MailboxEntry]) -> Vec<JournalRecord> {
    let mut ordered: Vec<&MailboxEntry> = entries.iter().collect();
    ordered.sort_by_key(|entry| entry.seq);
    let mut seen: Vec<&str> = Vec::with_capacity(ordered.len());
    let mut records = Vec::with_capacity(ordered.len());
    for entry in ordered {
        if seen.contains(&entry.idempotency_key.as_str()) {
            continue;
        }
        seen.push(&entry.idempotency_key);
        records.push(JournalRecord::UserMessage {
            content: entry.content.clone(),
            origin: MessageOrigin::ParentMessage,
        });
    }
    records
}

/// The digest a mailbox entry is stored under, so a redelivered send is a no-op.
#[must_use]
pub fn entry_digest(idempotency_key: &str) -> ContentHash {
    ContentHash::of(idempotency_key.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::{MailboxEntry, drain, entry_digest};
    use aex_brain_domain::ids::Timestamp;
    use aex_brain_domain::journal::{JournalRecord, MessageOrigin};
    use aex_brain_domain::wire_pending::{CanonicalBlock, ContentBlockRef};
    use aex_model_catalog::BoundedString;

    fn entry(seq: u64, key: &str, text: &str) -> MailboxEntry {
        MailboxEntry {
            seq,
            content: vec![ContentBlockRef::Inline {
                block: CanonicalBlock::Text {
                    text: BoundedString::truncating(text),
                    annotations: Vec::new(),
                },
            }],
            idempotency_key: key.to_owned(),
            sent_at: Timestamp::from_millis(i64::try_from(seq).unwrap_or(i64::MAX)),
        }
    }

    fn texts(records: &[JournalRecord]) -> Vec<String> {
        records
            .iter()
            .filter_map(|record| match record {
                JournalRecord::UserMessage { content, .. } => {
                    content.first().map(|block| match block {
                        ContentBlockRef::Inline {
                            block: CanonicalBlock::Text { text, .. },
                        } => text.as_str().to_owned(),
                        _ => "other".to_owned(),
                    })
                }
                _ => None,
            })
            .collect()
    }

    /// Mailbox order, not arrival order. A child that saw its parent's messages shuffled
    /// would answer a conversation that never happened.
    #[test]
    fn delivery_is_in_mailbox_order_whatever_order_the_rows_arrive_in() {
        let entries = [
            entry(2, "b", "second"),
            entry(0, "a", "first"),
            entry(1, "c", "middle"),
        ];
        assert_eq!(
            texts(&drain(&entries)),
            vec!["first".to_owned(), "middle".to_owned(), "second".to_owned()]
        );
    }

    /// Duplicates collapse on the caller's key. Keeping the first preserves the order the
    /// parent sent in; keeping the last would let a retry jump ahead of later messages.
    #[test]
    fn a_duplicate_send_collapses_onto_its_first_occurrence() {
        let entries = [
            entry(0, "a", "first"),
            entry(1, "b", "second"),
            entry(2, "a", "first retried"),
        ];
        assert_eq!(
            texts(&drain(&entries)),
            vec!["first".to_owned(), "second".to_owned()]
        );
    }

    /// Two identical texts under different keys are two messages: only the caller can say
    /// whether a repeat was deliberate.
    #[test]
    fn identical_text_under_two_keys_is_two_messages() {
        let entries = [entry(0, "a", "again"), entry(1, "b", "again")];
        assert_eq!(drain(&entries).len(), 2);
    }

    #[test]
    fn every_delivered_record_is_a_parent_message() {
        let records = drain(&[entry(0, "a", "hello")]);
        assert!(matches!(
            records[0],
            JournalRecord::UserMessage {
                origin: MessageOrigin::ParentMessage,
                ..
            }
        ));
    }

    #[test]
    fn an_empty_mailbox_delivers_nothing() {
        assert!(drain(&[]).is_empty());
    }

    #[test]
    fn the_stored_digest_is_a_function_of_the_callers_key() {
        assert_eq!(entry_digest("k"), entry_digest("k"));
        assert_ne!(entry_digest("k"), entry_digest("k2"));
    }
}
