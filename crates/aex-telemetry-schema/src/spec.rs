//! The const-friendly view of the registry used by [`crate::generated`].
//!
//! These types hold `&'static str` so the whole registry can be a `const` and so
//! the reserved-name rules can be asserted at compile time. The owned, parsed
//! view used by the generator lives in [`crate::registry`].

use serde::Deserialize;

/// How many distinct values an attribute may take.
///
/// The class is a policy statement, not a measurement: an `Unbounded` attribute
/// may never be used as a metric dimension, whatever its observed cardinality
/// happens to be in one deployment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cardinality {
    /// A closed, enumerable set fixed at build time.
    Fixed,
    /// An open set with a declared maximum length and a review obligation.
    Bounded,
    /// A per-entity identifier that must never become a metric dimension.
    Unbounded,
}

impl Cardinality {
    /// The `snake_case` name used in `registry.toml` and in generated source.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fixed => "fixed",
            Self::Bounded => "bounded",
            Self::Unbounded => "unbounded",
        }
    }

    /// The Rust variant path used in generated source.
    #[must_use]
    pub const fn variant(self) -> &'static str {
        match self {
            Self::Fixed => "Cardinality::Fixed",
            Self::Bounded => "Cardinality::Bounded",
            Self::Unbounded => "Cardinality::Unbounded",
        }
    }
}

/// Who may see an attribute value once a record leaves the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// Safe to export from the process.
    Public,
    /// Removed on export; usable only inside the emitting process.
    Internal,
    /// Never representable in an exported record at all.
    Forbidden,
}

impl Visibility {
    /// The `snake_case` name used in `registry.toml` and in generated source.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Internal => "internal",
            Self::Forbidden => "forbidden",
        }
    }

    /// The Rust variant path used in generated source.
    #[must_use]
    pub const fn variant(self) -> &'static str {
        match self {
            Self::Public => "Visibility::Public",
            Self::Internal => "Visibility::Internal",
            Self::Forbidden => "Visibility::Forbidden",
        }
    }

    /// Whether a value carrying this class may leave the process.
    #[must_use]
    pub const fn is_exportable(self) -> bool {
        matches!(self, Self::Public)
    }
}

/// The instrument kind of a declared metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Instrument {
    /// Monotonically increasing count.
    Counter,
    /// Count that may decrease.
    UpDownCounter,
    /// Distribution of recorded values.
    Histogram,
    /// Last observed value.
    Gauge,
}

impl Instrument {
    /// The `snake_case` name used in `registry.toml` and in generated source.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Counter => "counter",
            Self::UpDownCounter => "up_down_counter",
            Self::Histogram => "histogram",
            Self::Gauge => "gauge",
        }
    }

    /// The Rust variant path used in generated source.
    #[must_use]
    pub const fn variant(self) -> &'static str {
        match self {
            Self::Counter => "Instrument::Counter",
            Self::UpDownCounter => "Instrument::UpDownCounter",
            Self::Histogram => "Instrument::Histogram",
            Self::Gauge => "Instrument::Gauge",
        }
    }
}

/// One declared attribute and its two policy classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttributeSpec {
    /// Dotted attribute name as it appears on the wire.
    pub name: &'static str,
    /// How many distinct values this attribute may take.
    pub cardinality: Cardinality,
    /// Who may see this attribute once a record leaves the process.
    pub visibility: Visibility,
    /// Maximum permitted rendered length in bytes.
    pub max_len: usize,
}

/// One declared instrument and the attributes it may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetricSpec {
    /// Dotted metric name.
    pub name: &'static str,
    /// Instrument kind.
    pub instrument: Instrument,
    /// `UCUM` unit string.
    pub unit: &'static str,
    /// Attribute names this metric may be dimensioned by.
    pub attributes: &'static [&'static str],
}

/// One declared span and the attributes it may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpanSpec {
    /// Dotted span name.
    pub name: &'static str,
    /// Attribute names this span may carry.
    pub attributes: &'static [&'static str],
}

/// One declared event and the attributes it may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventSpec {
    /// Dotted event name.
    pub name: &'static str,
    /// Attribute names this event may carry.
    pub attributes: &'static [&'static str],
}
