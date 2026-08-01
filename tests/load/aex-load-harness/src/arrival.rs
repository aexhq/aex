//! The arrival process.
//!
//! Open arrival offers work on a schedule regardless of what the system under
//! test is doing, which is the only way to observe queueing and rejection.
//! Closed arrival keeps a fixed number of agents in flight, which is what the
//! Area 10 "peak active agents" rows describe. Both are pure functions of the
//! descriptor and a seed, so a campaign replays exactly and a shrunk failure is
//! reproducible.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A small, explicit, reproducible generator.
///
/// `SplitMix64`. It is written out rather than pulled in so the arrival
/// sequence is stable across dependency bumps: a campaign compared against last
/// week's campaign must have offered the same work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rng(u64);

impl Rng {
    /// A generator seeded with `seed`.
    #[must_use]
    pub const fn seeded(seed: u64) -> Self {
        Self(seed)
    }

    /// The next 64 bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// The next value in `[0, 1)`.
    pub fn next_unit(&mut self) -> f64 {
        // 53 significant bits keeps every value exactly representable.
        #[allow(clippy::cast_precision_loss)]
        {
            (self.next_u64() >> 11) as f64 / (1_u64 << 53) as f64
        }
    }
}

/// The relative weights of the workload's request shapes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Mix(pub BTreeMap<String, f64>);

impl Mix {
    /// The tolerance the declared weights must sum to 1.0 within.
    pub const TOLERANCE: f64 = 1e-9;

    /// Whether the weights sum to 1.0.
    #[must_use]
    pub fn is_normalized(&self) -> bool {
        (self.0.values().sum::<f64>() - 1.0).abs() <= Self::TOLERANCE
    }

    /// The shape drawn for `value` in `[0, 1)`, by cumulative weight over the
    /// sorted shape names.
    ///
    /// # Panics
    ///
    /// Panics when the mix is empty, because a workload with no shapes offers
    /// nothing and would otherwise silently record a green run of zero work.
    #[must_use]
    pub fn draw(&self, value: f64) -> &str {
        assert!(
            !self.0.is_empty(),
            "a workload mix declares at least one shape"
        );
        let mut cumulative = 0.0;
        let mut last = "";
        for (shape, weight) in &self.0 {
            cumulative += weight;
            last = shape;
            if value < cumulative {
                return shape;
            }
        }
        last
    }
}

/// How work is offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arrival {
    /// A fixed number of agents in flight; a new offer waits for a completion.
    Closed,
    /// Offers on a schedule, whatever the system under test is doing.
    Open,
}

/// One offered unit of work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    /// Zero-based sequence number.
    pub seq: u64,
    /// Milliseconds after the campaign start at which this offer is due. Always
    /// zero for closed arrival, where the previous completion is the trigger.
    pub due_at_ms: u64,
    /// The mix shape drawn for this offer.
    pub shape: String,
}

/// The full offered timeline of a campaign.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrivalSchedule {
    /// The arrival mode.
    pub arrival: Arrival,
    /// Agents in flight for closed arrival; the target rate driver for open.
    pub concurrency: u32,
    /// The offers, in due order.
    pub offers: Vec<Offer>,
}

impl ArrivalSchedule {
    /// Builds the schedule for a closed campaign: `concurrency` offers are in
    /// flight at any instant and `total` offers are made overall.
    #[must_use]
    pub fn closed(concurrency: u32, total: u64, mix: &Mix, seed: u64) -> Self {
        let mut rng = Rng::seeded(seed);
        let offers = (0..total)
            .map(|seq| Offer {
                seq,
                due_at_ms: 0,
                shape: mix.draw(rng.next_unit()).to_owned(),
            })
            .collect();
        Self {
            arrival: Arrival::Closed,
            concurrency,
            offers,
        }
    }

    /// Builds the schedule for an open campaign of `duration_ms` at
    /// `rate_per_s`, with exponential inter-arrival times.
    #[must_use]
    pub fn open(rate_per_s: f64, duration_ms: u64, mix: &Mix, seed: u64) -> Self {
        let mut rng = Rng::seeded(seed);
        let mut offers = Vec::new();
        let mut now_ms = 0.0_f64;
        let mut seq = 0_u64;
        #[allow(clippy::cast_precision_loss)]
        let horizon = duration_ms as f64;
        while now_ms < horizon {
            // Inverse-transform sampling of the exponential distribution.
            let unit = 1.0 - rng.next_unit();
            let gap_ms = -unit.ln() / rate_per_s * 1_000.0;
            now_ms += gap_ms;
            if now_ms >= horizon {
                break;
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            offers.push(Offer {
                seq,
                due_at_ms: now_ms as u64,
                shape: mix.draw(rng.next_unit()).to_owned(),
            });
            seq += 1;
        }
        Self {
            arrival: Arrival::Open,
            concurrency: 0,
            offers,
        }
    }

    /// How many offers the campaign makes.
    #[must_use]
    pub fn offered(&self) -> usize {
        self.offers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{Arrival, ArrivalSchedule, Mix, Rng};
    use std::collections::BTreeMap;

    fn mix() -> Mix {
        Mix(BTreeMap::from([
            ("long_context_1mib".to_owned(), 0.05),
            ("short_turn".to_owned(), 0.70),
            ("subagent_fanout".to_owned(), 0.10),
            ("tool_heavy".to_owned(), 0.15),
        ]))
    }

    #[test]
    fn a_declared_mix_sums_to_one() {
        assert!(mix().is_normalized());
        assert!(!Mix(BTreeMap::from([("a".to_owned(), 0.5)])).is_normalized());
    }

    #[test]
    fn a_draw_selects_by_cumulative_weight_over_sorted_names() {
        let mix = mix();
        assert_eq!(mix.draw(0.0), "long_context_1mib");
        assert_eq!(mix.draw(0.04), "long_context_1mib");
        assert_eq!(mix.draw(0.06), "short_turn");
        assert_eq!(mix.draw(0.80), "subagent_fanout");
        assert_eq!(mix.draw(0.90), "tool_heavy");
        assert_eq!(mix.draw(0.999_999), "tool_heavy");
    }

    #[test]
    fn the_same_seed_replays_the_same_campaign() {
        let first = ArrivalSchedule::open(10.0, 5_000, &mix(), 42);
        let second = ArrivalSchedule::open(10.0, 5_000, &mix(), 42);
        assert_eq!(first, second);
        let different = ArrivalSchedule::open(10.0, 5_000, &mix(), 43);
        assert_ne!(first.offers, different.offers);
    }

    #[test]
    fn an_open_schedule_is_due_in_order_and_inside_the_horizon() {
        let schedule = ArrivalSchedule::open(50.0, 2_000, &mix(), 7);
        assert_eq!(schedule.arrival, Arrival::Open);
        assert!(schedule.offered() > 50, "offered {}", schedule.offered());
        let mut previous = 0;
        for offer in &schedule.offers {
            assert!(offer.due_at_ms >= previous, "{offer:?}");
            assert!(offer.due_at_ms < 2_000, "{offer:?}");
            previous = offer.due_at_ms;
        }
    }

    #[test]
    fn a_closed_schedule_offers_exactly_the_requested_total() {
        let schedule = ArrivalSchedule::closed(100, 250, &mix(), 1);
        assert_eq!(schedule.arrival, Arrival::Closed);
        assert_eq!(schedule.concurrency, 100);
        assert_eq!(schedule.offered(), 250);
        assert!(schedule.offers.iter().all(|offer| offer.due_at_ms == 0));
    }

    #[test]
    fn the_generator_stays_inside_the_unit_interval() {
        let mut rng = Rng::seeded(0xDEAD_BEEF);
        for _ in 0..10_000 {
            let value = rng.next_unit();
            assert!((0.0..1.0).contains(&value), "{value}");
        }
    }
}
