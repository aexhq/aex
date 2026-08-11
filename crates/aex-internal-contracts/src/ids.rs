//! Identifiers that are meaningful only between internal execution processes.

use std::fmt;
use std::str::FromStr;

use aex_wire::ids::{IdParseError, IdText, Uuid7};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// One admitted unit of internal execution.
///
/// The `run_` spelling remains stable in journals and database keys, but this
/// identity is deliberately absent from the public ID registry, HTTP schemas,
/// SDK and CLI. Public callers observe only session state and sealed messages.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunId(Uuid7);

impl RunId {
    /// The private persisted prefix, without the underscore.
    pub const PREFIX: &'static str = "run";

    /// Builds an internal run identity from a validated `UUIDv7` payload.
    #[must_use]
    pub const fn from_uuid7(value: Uuid7) -> Self {
        Self(value)
    }

    /// Returns the validated `UUIDv7` payload.
    #[must_use]
    pub const fn uuid7(&self) -> Uuid7 {
        self.0
    }

    /// Renders the stable private text without allocating.
    #[must_use]
    pub const fn encode(&self) -> IdText {
        IdText::new(Self::PREFIX, &self.0.encode_suffix())
    }

    /// Parses the exact private `run_<uuidv7>` spelling.
    ///
    /// # Errors
    ///
    /// Returns [`IdParseError`] for a wrong prefix, malformed suffix or a UUID
    /// payload that is not version 7.
    pub fn parse(text: &str) -> Result<Self, IdParseError> {
        let Some(suffix) = text
            .strip_prefix(Self::PREFIX)
            .and_then(|rest| rest.strip_prefix('_'))
        else {
            let found = text.split_once('_').map_or(text, |(head, _)| head);
            return Err(IdParseError::WrongPrefix {
                expected: Self::PREFIX,
                found: found.into(),
            });
        };
        Uuid7::decode_suffix(suffix.as_bytes()).map(Self)
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.encode().as_str())
    }
}

impl fmt::Debug for RunId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "RunId({self})")
    }
}

impl FromStr for RunId {
    type Err = IdParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

impl Serialize for RunId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.encode().as_str())
    }
}

impl<'de> Deserialize<'de> for RunId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        Self::parse(text.as_ref()).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::RunId;
    use aex_wire::ids::Uuid7;

    #[test]
    fn the_private_identity_keeps_its_persisted_spelling() {
        let id = RunId::from_uuid7(Uuid7::compose(1, [7; 10]));
        let text = id.to_string();
        assert!(text.starts_with("run_"));
        assert_eq!(RunId::parse(&text), Ok(id));
        assert!(RunId::parse(&text.replacen("run_", "ses_", 1)).is_err());
        assert_eq!(
            serde_json::to_string(&id).expect("serializes"),
            format!("\"{text}\"")
        );
    }
}
