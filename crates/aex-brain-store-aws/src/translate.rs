//! The one seam between `aex-brain-domain`'s identifiers and the wire identifiers the
//! regional key templates take.
//!
//! `aex-brain-domain` is deliberately dependency-free, so it carries plain `Uuid` newtypes;
//! `aex-session-dynamodb::keys` takes `aex_wire::ids::*`, which are version-7 payloads with
//! a prefix. Every crossing goes through this module, and every crossing is **fallible**: a
//! value that is not a valid version-7 payload is a typed refusal rather than a key built
//! out of a nonsense identifier.
//!
//! There is exactly one conversion per direction per type. A second one somewhere else in
//! the crate would be a second chance to render an identifier differently, and a key is
//! only useful if two writers agree on it byte for byte.

use aex_brain_domain::ids as brain;
use aex_wire::ids::{PrefixedId, Uuid7};
use aex_wire::types::Timestamp as WireTimestamp;

/// Why a Brain value could not be rendered as a wire value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TranslateError {
    /// The identifier is not a version-7 payload, so it cannot become a prefixed id.
    #[error("`{what}` is not a valid version-7 identifier payload")]
    NotUuid7 {
        /// Which identifier.
        what: &'static str,
    },
    /// The instant is outside the wire timestamp range.
    #[error("`{what}` is {millis} ms from the epoch, outside the representable range")]
    TimestampRange {
        /// Which instant.
        what: &'static str,
        /// The offending value.
        millis: i64,
    },
}

fn uuid7(value: uuid::Uuid, what: &'static str) -> Result<Uuid7, TranslateError> {
    Uuid7::from_bytes(*value.as_bytes()).map_err(|_| TranslateError::NotUuid7 { what })
}

/// The wire session identifier.
///
/// # Errors
///
/// [`TranslateError::NotUuid7`] when the payload is not version 7.
pub fn session(value: brain::SessionId) -> Result<aex_wire::ids::SessionId, TranslateError> {
    Ok(aex_wire::ids::SessionId::from_uuid7(uuid7(
        value.0, "session",
    )?))
}

/// The wire agent identifier.
///
/// # Errors
///
/// [`TranslateError::NotUuid7`] when the payload is not version 7.
pub fn agent(value: brain::AgentId) -> Result<aex_wire::ids::AgentId, TranslateError> {
    Ok(aex_wire::ids::AgentId::from_uuid7(uuid7(value.0, "agent")?))
}

/// Both halves of an agent key.
///
/// # Errors
///
/// As [`session`] and [`agent`].
pub fn agent_key(
    key: &brain::AgentKey,
) -> Result<(aex_wire::ids::SessionId, aex_wire::ids::AgentId), TranslateError> {
    Ok((session(key.session)?, agent(key.agent)?))
}

/// The wire instant.
///
/// # Errors
///
/// [`TranslateError::TimestampRange`] when the instant is outside the wire range.
pub fn at(value: brain::Timestamp, what: &'static str) -> Result<WireTimestamp, TranslateError> {
    WireTimestamp::from_unix_millis(value.millis()).map_err(|_| TranslateError::TimestampRange {
        what,
        millis: value.millis(),
    })
}

/// The Brain instant a wire instant denotes.
#[must_use]
pub const fn from_wire(value: WireTimestamp) -> brain::Timestamp {
    brain::Timestamp::from_millis(value.unix_millis())
}

/// A Brain agent identifier read back out of a stored wire identifier.
#[must_use]
pub fn agent_from_wire(value: aex_wire::ids::AgentId) -> brain::AgentId {
    brain::AgentId(uuid::Uuid::from_bytes(*value.uuid7().as_bytes()))
}

/// A Brain session identifier read back out of a stored wire identifier.
#[must_use]
pub fn session_from_wire(value: aex_wire::ids::SessionId) -> brain::SessionId {
    brain::SessionId(uuid::Uuid::from_bytes(*value.uuid7().as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::{
        TranslateError, agent, agent_from_wire, at, from_wire, session, session_from_wire,
    };
    use aex_brain_domain::ids as brain;
    use aex_wire::ids::Uuid7;

    fn v7(millis: u64, seed: u8) -> uuid::Uuid {
        uuid::Uuid::from_bytes(*Uuid7::compose(millis, [seed; 10]).as_bytes())
    }

    #[test]
    fn a_version_seven_payload_round_trips_through_the_wire_form() {
        let original = brain::AgentId(v7(1_767_225_600_000, 7));
        let wire = agent(original).expect("a version-7 payload converts");
        assert_eq!(agent_from_wire(wire), original);

        let original = brain::SessionId(v7(1_767_225_600_001, 8));
        let wire = session(original).expect("a version-7 payload converts");
        assert_eq!(session_from_wire(wire), original);
    }

    /// A key built from a nonsense identifier is worse than no key: it addresses a
    /// partition nothing else will ever address, so the write succeeds and is invisible.
    #[test]
    fn a_non_version_seven_identifier_is_refused_rather_than_rendered() {
        let error = agent(brain::AgentId(uuid::Uuid::from_u128(1)))
            .expect_err("a version-4-shaped payload has no wire form");
        assert_eq!(error, TranslateError::NotUuid7 { what: "agent" });
    }

    #[test]
    fn a_child_identity_derived_by_the_domain_is_always_convertible() {
        let parent = brain::AgentId(v7(1_767_225_600_000, 3));
        for ordinal in 0..64_u32 {
            let child = aex_brain_domain::ids::child_agent_id(parent, ordinal);
            assert!(
                agent(child).is_ok(),
                "the domain derives version-7 child ids so a fanout page can always key them"
            );
        }
    }

    #[test]
    fn an_instant_outside_the_wire_range_is_a_typed_refusal() {
        let error = at(brain::Timestamp::from_millis(i64::MIN), "deadline")
            .expect_err("the minimum instant has no wire spelling");
        assert!(
            matches!(
                error,
                TranslateError::TimestampRange {
                    what: "deadline",
                    ..
                }
            ),
            "{error:?}"
        );
    }

    #[test]
    fn an_instant_inside_the_range_round_trips_exactly() {
        let original = brain::Timestamp::from_millis(1_767_225_600_123);
        let wire = at(original, "now").expect("in range");
        assert_eq!(from_wire(wire), original);
    }
}
