//! Messages.
//!
//! `Open -> Sealed` is one-way (D-13). A user message is born `Sealed`; only the
//! agent holding the current [`AgentFence`] may append to an open one; sealing is
//! idempotent, and the terminal barrier seals every open message of its run so a
//! torn assistant message can never be listed.

use aex_content_domain::ContentDigest;
use aex_wire::ids::{AgentId, MessageId, RunId, SessionId, ToolCallId};
use aex_wire::types::Timestamp;

use crate::ids::AgentFence;

/// Who a message is from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MessageRole {
    /// The customer.
    User,
    /// The model.
    Assistant,
    /// A tool result.
    Tool,
}

impl MessageRole {
    /// Every role, in canonical order.
    pub const ALL: [Self; 3] = [Self::User, Self::Assistant, Self::Tool];

    /// Whether a message of this role is born sealed.
    #[must_use]
    pub const fn born_sealed(self) -> bool {
        matches!(self, Self::User)
    }
}

/// Whether a message may still grow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MessageState {
    /// Parts may still be appended.
    Open,
    /// Immutable.
    Sealed,
}

/// One piece of a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessagePart {
    /// Text.
    Text {
        /// The text.
        text: String,
    },
    /// A tool call the model asked for.
    ToolCall {
        /// Which call.
        id: ToolCallId,
        /// The canonical argument digest.
        arguments: ContentDigest,
    },
    /// A tool result.
    ToolResult {
        /// Which call it answers.
        id: ToolCallId,
        /// The canonical result digest.
        result: ContentDigest,
    },
}

/// One message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// Its identity.
    pub id: MessageId,
    /// The owning session.
    pub session: SessionId,
    /// The run that produced it, when one did.
    pub run: Option<RunId>,
    /// The agent that produced it.
    pub agent: AgentId,
    /// Who it is from.
    pub role: MessageRole,
    /// Whether it may still grow.
    pub state: MessageState,
    /// Its parts, in order.
    pub parts: Vec<MessagePart>,
    /// When it was created.
    pub created_at: Timestamp,
    /// When it was sealed.
    pub sealed_at: Option<Timestamp>,
}

/// What an append or seal changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageDelta {
    /// The message after the change.
    pub message: Message,
    /// Whether anything actually changed.
    pub changed: bool,
}

/// Why a message change was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MessageError {
    /// The message is sealed.
    #[error("message is sealed and can never change again")]
    Sealed,
    /// The caller presented a stale fence.
    #[error("presented fence {presented} is not the message's {current}")]
    StaleFence {
        /// The fence the message's agent is at.
        current: AgentFence,
        /// What the caller presented.
        presented: AgentFence,
    },
}

/// Appends a part to an open message.
///
/// # Errors
///
/// Returns [`MessageError::Sealed`] for a sealed message and
/// [`MessageError::StaleFence`] when the caller is not the current owner.
pub fn append_part(
    message: &Message,
    current_fence: AgentFence,
    part: MessagePart,
    presented: AgentFence,
) -> Result<MessageDelta, MessageError> {
    if message.state == MessageState::Sealed {
        return Err(MessageError::Sealed);
    }
    if presented != current_fence {
        return Err(MessageError::StaleFence {
            current: current_fence,
            presented,
        });
    }
    let mut next = message.clone();
    next.parts.push(part);
    Ok(MessageDelta {
        message: next,
        changed: true,
    })
}

/// Seals a message.
///
/// Idempotent: sealing an already sealed message reports `changed = false` and
/// keeps the original instant, so the terminal barrier can seal every open
/// message of its run without knowing which were already sealed.
#[must_use]
pub fn seal(message: &Message, now: Timestamp) -> MessageDelta {
    if message.state == MessageState::Sealed {
        return MessageDelta {
            message: message.clone(),
            changed: false,
        };
    }
    let mut next = message.clone();
    next.state = MessageState::Sealed;
    next.sealed_at = Some(now);
    MessageDelta {
        message: next,
        changed: true,
    }
}

#[cfg(test)]
mod tests {
    use aex_content_domain::ContentDigest;
    use aex_wire::ids::{AgentId, MessageId, PrefixedId as _, SessionId, Uuid7};
    use aex_wire::types::Timestamp;

    use super::{Message, MessageError, MessagePart, MessageRole, MessageState, append_part, seal};
    use crate::ids::AgentFence;

    fn moment(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    fn open(role: MessageRole) -> Message {
        Message {
            id: MessageId::from_uuid7(Uuid7::compose(1, [1; 10])),
            session: SessionId::from_uuid7(Uuid7::compose(1, [2; 10])),
            run: None,
            agent: AgentId::from_uuid7(Uuid7::compose(1, [3; 10])),
            role,
            state: MessageState::Open,
            parts: Vec::new(),
            created_at: moment(0),
            sealed_at: None,
        }
    }

    fn text() -> MessagePart {
        MessagePart::Text {
            text: "hello".to_owned(),
        }
    }

    #[test]
    fn only_the_current_fence_may_append() {
        let message = open(MessageRole::Assistant);
        assert_eq!(
            append_part(&message, AgentFence(4), text(), AgentFence(3)),
            Err(MessageError::StaleFence {
                current: AgentFence(4),
                presented: AgentFence(3)
            })
        );
        let delta = append_part(&message, AgentFence(4), text(), AgentFence(4)).expect("appends");
        assert!(delta.changed);
        assert_eq!(delta.message.parts.len(), 1);
    }

    #[test]
    fn sealing_is_one_way_and_idempotent() {
        let message = open(MessageRole::Assistant);
        let sealed = seal(&message, moment(5));
        assert!(sealed.changed);
        assert_eq!(sealed.message.state, MessageState::Sealed);
        assert_eq!(sealed.message.sealed_at, Some(moment(5)));

        let again = seal(&sealed.message, moment(9));
        assert!(!again.changed);
        assert_eq!(again.message.sealed_at, Some(moment(5)));

        assert_eq!(
            append_part(&sealed.message, AgentFence(1), text(), AgentFence(1)),
            Err(MessageError::Sealed)
        );
    }

    #[test]
    fn a_user_message_is_born_sealed() {
        assert!(MessageRole::User.born_sealed());
        assert!(!MessageRole::Assistant.born_sealed());
        assert!(!MessageRole::Tool.born_sealed());
    }

    #[test]
    fn a_tool_part_carries_its_call_identity() {
        let digest = ContentDigest::of(b"{}");
        let message = open(MessageRole::Tool);
        let part = MessagePart::ToolResult {
            id: aex_wire::ids::ToolCallId::from_uuid7(Uuid7::compose(1, [4; 10])),
            result: digest,
        };
        let delta =
            append_part(&message, AgentFence(0), part.clone(), AgentFence(0)).expect("appends");
        assert_eq!(delta.message.parts, vec![part]);
    }
}
