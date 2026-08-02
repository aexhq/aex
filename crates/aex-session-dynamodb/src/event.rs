//! Read-only native session-event projection.
//!
//! This decoder is feature-neutral across the authority and projection feature
//! sets so observation readers can consume the one session-event authority
//! without linking transaction builders or write codecs.

use crate::attr::{CodecError, Item, Row};
use crate::wire_pending::{Body, SessionEvent};

/// The `itemType` of a native event.
pub const SESSION_EVENT: &str = "session_event";
/// The attribute an inline event body lives under.
pub const BODY_INLINE: &str = "bodyInline";
/// The attribute a referenced event body lives under.
pub const BODY_DIGEST: &str = "bodyDigest";
/// Every native-event outbox state.
pub const OUTBOX_STATES: &[&str] = &["pending", "delivered"];

/// Decodes one native event without coercion or defaults.
///
/// # Errors
///
/// Returns [`CodecError`] for the wrong row family, a missing/malformed field,
/// an invalid observation identity, or an absent body placement.
pub fn decode(item: &Item) -> Result<SessionEvent, CodecError> {
    let row = Row::bind(item, SESSION_EVENT)?;
    let event_id = row
        .string("eventId")?
        .parse::<aex_wire::ids::ObservationId>()
        .map_err(|error| CodecError::Malformed {
            item_type: SESSION_EVENT,
            attribute: "eventId",
            reason: error.to_string(),
        })?;
    let body = if let Some(bytes) = row.opt_bytes(BODY_INLINE)? {
        Body::Inline(bytes.to_vec())
    } else if let Some(reference) = row.opt_string(BODY_DIGEST)? {
        Body::Digest(reference.to_owned())
    } else {
        return Err(CodecError::Malformed {
            item_type: SESSION_EVENT,
            attribute: BODY_INLINE,
            reason: "exactly one inline body or content digest is required".to_owned(),
        });
    };
    Ok(SessionEvent {
        workspace: row.id("workspaceId")?,
        event_seq: row.u64("eventSeq")?,
        event_id,
        event_type: row.string("type")?.to_owned(),
        run: row.opt_id("runId")?,
        agent: row.opt_id("agentId")?,
        body,
        occurred_at: row.timestamp("occurredAt")?,
        outbox_state: row.enumerated("outboxState", OUTBOX_STATES)?,
    })
}
