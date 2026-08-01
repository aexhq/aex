//! The five observable signals and their bounded sets.
//!
//! `events` are deliberately part of the ordering vocabulary but are **not**
//! stored in `observation-authority`: they are the session journal and are read
//! through a `SemanticEventSource` port (decision O-01). Anything that formats a
//! storage key checks [`Signal::in_observation_authority`] first.

use aex_wire::models::ObservationSignal;

/// One observable signal.
///
/// Declaration order is the rank order, and the rank is the second component of
/// the ordering tuple, so it is a wire-visible ordering fact and never changes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Signal {
    /// Structured platform events, stored in `session-authority`.
    Events,
    /// Log records.
    Logs,
    /// Individual spans.
    Spans,
    /// Metric points.
    Metrics,
    /// Assembled trace summaries.
    Traces,
}

impl Signal {
    /// Every signal, in rank order.
    pub const ALL: &'static [Signal] = &[
        Signal::Events,
        Signal::Logs,
        Signal::Spans,
        Signal::Metrics,
        Signal::Traces,
    ];

    /// The four signals that live in `observation-authority`.
    pub const AUTHORITY: &'static [Signal] =
        &[Signal::Logs, Signal::Spans, Signal::Metrics, Signal::Traces];

    /// The rank used as the second component of the ordering tuple.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Events => 0,
            Self::Logs => 1,
            Self::Spans => 2,
            Self::Metrics => 3,
            Self::Traces => 4,
        }
    }

    /// The literal lowercase word used in every key and every wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Events => "events",
            Self::Logs => "logs",
            Self::Spans => "spans",
            Self::Metrics => "metrics",
            Self::Traces => "traces",
        }
    }

    /// Parses the literal lowercase word.
    ///
    /// `telemetry` is a *set* on the wire, not a signal, and is rejected here.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|s| s.as_str() == text)
    }

    /// Whether observations of this signal are stored in `observation-authority`.
    #[must_use]
    pub const fn in_observation_authority(self) -> bool {
        !matches!(self, Self::Events)
    }

    /// The wire spelling of this signal.
    #[must_use]
    pub const fn to_wire(self) -> ObservationSignal {
        match self {
            Self::Events => ObservationSignal::Events,
            Self::Logs => ObservationSignal::Logs,
            Self::Spans => ObservationSignal::Spans,
            Self::Metrics => ObservationSignal::Metrics,
            Self::Traces => ObservationSignal::Traces,
        }
    }
}

/// A bounded, ordered set of signals.
///
/// A bitset rather than a `Vec` because it is copied into every cursor binding
/// and compared on every page, and because it makes "the same set" a byte
/// comparison instead of an order-sensitive one.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SignalSet(u8);

impl SignalSet {
    /// The empty set.
    pub const EMPTY: Self = Self(0);

    /// Every signal, including `events`.
    #[must_use]
    pub const fn all() -> Self {
        Self(0b0001_1111)
    }

    /// The four signals stored in `observation-authority`.
    #[must_use]
    pub const fn authority() -> Self {
        Self(0b0001_1110)
    }

    /// The singleton set.
    #[must_use]
    pub const fn from_signal(signal: Signal) -> Self {
        Self(1 << signal.rank())
    }

    /// Adds a signal.
    #[must_use]
    pub const fn with(self, signal: Signal) -> Self {
        Self(self.0 | (1 << signal.rank()))
    }

    /// Whether the set holds `signal`.
    #[must_use]
    pub const fn contains(self, signal: Signal) -> bool {
        self.0 & (1 << signal.rank()) != 0
    }

    /// Whether the set holds nothing.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// How many signals the set holds.
    #[must_use]
    pub const fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    /// The set restricted to signals stored in `observation-authority`.
    #[must_use]
    pub const fn in_authority(self) -> Self {
        Self(self.0 & Self::authority().0)
    }

    /// Whether the two sets share a signal.
    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// The signals, in rank order.
    pub fn iter(self) -> impl Iterator<Item = Signal> {
        Signal::ALL
            .iter()
            .copied()
            .filter(move |signal| self.contains(*signal))
    }

    /// The raw bits, for a cursor binding or a digest input.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Expands the wire signal selector: `telemetry` means every signal.
    #[must_use]
    pub const fn from_wire(signal: ObservationSignal) -> Self {
        match signal {
            ObservationSignal::Events => Self::from_signal(Signal::Events),
            ObservationSignal::Logs => Self::from_signal(Signal::Logs),
            ObservationSignal::Spans => Self::from_signal(Signal::Spans),
            ObservationSignal::Metrics => Self::from_signal(Signal::Metrics),
            ObservationSignal::Traces => Self::from_signal(Signal::Traces),
            ObservationSignal::Telemetry => Self::all(),
        }
    }
}

impl IntoIterator for SignalSet {
    type Item = Signal;
    type IntoIter = std::vec::IntoIter<Signal>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter().collect::<Vec<_>>().into_iter()
    }
}

impl FromIterator<Signal> for SignalSet {
    fn from_iter<T: IntoIterator<Item = Signal>>(iter: T) -> Self {
        iter.into_iter().fold(Self::EMPTY, Self::with)
    }
}

#[cfg(test)]
mod tests {
    use super::{Signal, SignalSet};
    use aex_wire::models::ObservationSignal;

    #[test]
    fn only_events_live_outside_the_observation_authority() {
        assert!(!Signal::Events.in_observation_authority());
        for signal in Signal::AUTHORITY {
            assert!(signal.in_observation_authority());
        }
        assert_eq!(Signal::AUTHORITY.len(), 4);
    }

    #[test]
    fn the_set_round_trips_every_signal() {
        let set: SignalSet = Signal::ALL.iter().copied().collect();
        assert_eq!(set, SignalSet::all());
        assert_eq!(set.len(), 5);
        assert_eq!(set.iter().collect::<Vec<_>>(), Signal::ALL.to_vec());
        assert_eq!(SignalSet::all().in_authority(), SignalSet::authority());
        assert_eq!(SignalSet::authority().len(), 4);
    }

    #[test]
    fn telemetry_expands_to_every_signal() {
        assert_eq!(
            SignalSet::from_wire(ObservationSignal::Telemetry),
            SignalSet::all()
        );
        assert_eq!(
            SignalSet::from_wire(ObservationSignal::Logs),
            SignalSet::from_signal(Signal::Logs)
        );
    }

    #[test]
    fn the_empty_set_intersects_nothing() {
        assert!(SignalSet::EMPTY.is_empty());
        assert!(!SignalSet::EMPTY.intersects(SignalSet::all()));
        assert!(SignalSet::from_signal(Signal::Logs).intersects(SignalSet::all()));
    }
}
