//! The diagnostic record the facade accepts.
//!
//! A record names one registry-declared span, event or instrument and carries
//! registry-declared attributes. Nothing here decides whether a record is
//! exported; that is [`crate::facade::Handle`]'s job.

/// What kind of signal a [`Record`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RecordKind {
    /// A span from the registry's span list.
    Span,
    /// An event from the registry's event list.
    Event,
    /// A measurement against an instrument from the registry's metric list.
    Metric,
}

/// A recorded attribute value.
///
/// Deliberately narrow. There is no floating point variant, because a diagnostic
/// value that cannot be compared exactly makes a golden test ambiguous, and no
/// nested variant, because depth is what turns a diagnostic into an unbounded
/// payload.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AttributeValue {
    /// A textual value, bounded by the registry's declared maximum length.
    Text(String),
    /// A signed integer value.
    Integer(i64),
    /// A boolean value.
    Boolean(bool),
}

impl From<String> for AttributeValue {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for AttributeValue {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<i64> for AttributeValue {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}

impl From<u32> for AttributeValue {
    fn from(value: u32) -> Self {
        Self::Integer(i64::from(value))
    }
}

impl From<bool> for AttributeValue {
    fn from(value: bool) -> Self {
        Self::Boolean(value)
    }
}

/// One attribute of a [`Record`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    /// A registry-declared attribute name.
    pub key: &'static str,
    /// The recorded value.
    pub value: AttributeValue,
}

/// One diagnostic record awaiting export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// Which kind of signal this record describes.
    pub kind: RecordKind,
    /// A registry-declared span, event or instrument name.
    pub name: &'static str,
    /// The recorded attributes, in declaration order.
    pub attributes: Vec<Attribute>,
    /// The measurement, for [`RecordKind::Metric`] records.
    pub value: Option<i64>,
}

impl Record {
    /// Starts an event record for the registry-declared `name`.
    #[must_use]
    pub const fn event(name: &'static str) -> Self {
        Self {
            kind: RecordKind::Event,
            name,
            attributes: Vec::new(),
            value: None,
        }
    }

    /// Starts a span record for the registry-declared `name`.
    #[must_use]
    pub const fn span(name: &'static str) -> Self {
        Self {
            kind: RecordKind::Span,
            name,
            attributes: Vec::new(),
            value: None,
        }
    }

    /// Starts a measurement against the registry-declared instrument `name`.
    #[must_use]
    pub const fn metric(name: &'static str, value: i64) -> Self {
        Self {
            kind: RecordKind::Metric,
            name,
            attributes: Vec::new(),
            value: Some(value),
        }
    }

    /// Adds one attribute.
    ///
    /// The key must be a registry-declared attribute name. A key that is not
    /// declared public is removed when the record is emitted, so adding one here
    /// is safe but pointless.
    #[must_use]
    pub fn with(mut self, key: &'static str, value: impl Into<AttributeValue>) -> Self {
        self.attributes.push(Attribute {
            key,
            value: value.into(),
        });
        self
    }

    /// The value recorded for `key`, if the record carries it.
    #[must_use]
    pub fn attribute(&self, key: &str) -> Option<&AttributeValue> {
        self.attributes
            .iter()
            .find(|attribute| attribute.key == key)
            .map(|found| &found.value)
    }
}

#[cfg(test)]
mod tests {
    use super::{AttributeValue, Record, RecordKind};

    #[test]
    fn builders_set_the_expected_kind() {
        assert_eq!(Record::event("aex.process.started").kind, RecordKind::Event);
        assert_eq!(Record::span("aex.provider.call").kind, RecordKind::Span);
        let metric = Record::metric("aex.operation.duration", 12);
        assert_eq!(metric.kind, RecordKind::Metric);
        assert_eq!(metric.value, Some(12));
    }

    #[test]
    fn attributes_are_retrievable_by_key() {
        let record = Record::event("aex.process.started")
            .with("aex.plane", "dev")
            .with("aex.retry.count", 3_i64)
            .with("aex.outcome", true);
        assert_eq!(
            record.attribute("aex.plane"),
            Some(&AttributeValue::Text("dev".to_owned()))
        );
        assert_eq!(
            record.attribute("aex.retry.count"),
            Some(&AttributeValue::Integer(3))
        );
        assert_eq!(
            record.attribute("aex.outcome"),
            Some(&AttributeValue::Boolean(true))
        );
        assert_eq!(record.attribute("aex.absent"), None);
    }
}
