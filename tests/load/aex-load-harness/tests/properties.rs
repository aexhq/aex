//! Generated properties over the load harness invariants.
//!
//! Three things must hold for every campaign shape, not only the ones a
//! hand-written case reaches: the arrival process is a pure function of its
//! inputs, the recorder's percentiles are ordered and bounded by the sample,
//! and a gate report never calls an unreported gate a pass.

use aex_load_harness::{
    ArrivalSchedule, Gate, GateKind, GateOutcome, GateReport, GateVerdict, Metric, Mix, Recorder,
    Rng,
};
use proptest::prelude::*;
use std::collections::BTreeMap;

fn mix_strategy() -> impl Strategy<Value = Mix> {
    prop::collection::vec(1_u32..100, 1..6).prop_map(|weights| {
        let total = f64::from(weights.iter().sum::<u32>());
        let map: BTreeMap<String, f64> = weights
            .iter()
            .enumerate()
            .map(|(index, weight)| (format!("shape_{index}"), f64::from(*weight) / total))
            .collect();
        Mix(map)
    })
}

proptest! {
    /// The same seed always replays the same campaign, so a capacity result is
    /// reproducible and a regression is attributable.
    #[test]
    fn an_open_schedule_is_a_pure_function_of_its_inputs(
        seed in any::<u64>(),
        rate in 1_u32..500,
        duration_ms in 100_u64..5_000,
        mix in mix_strategy(),
    ) {
        let first = ArrivalSchedule::open(f64::from(rate), duration_ms, &mix, seed);
        let second = ArrivalSchedule::open(f64::from(rate), duration_ms, &mix, seed);
        prop_assert_eq!(&first, &second);
        for offer in &first.offers {
            prop_assert!(offer.due_at_ms < duration_ms);
            prop_assert!(mix.0.contains_key(&offer.shape), "{}", offer.shape);
        }
    }

    /// A closed schedule offers exactly what was asked for, whatever the mix.
    #[test]
    fn a_closed_schedule_offers_exactly_the_total(
        seed in any::<u64>(),
        concurrency in 1_u32..64,
        total in 0_u64..200,
        mix in mix_strategy(),
    ) {
        let schedule = ArrivalSchedule::closed(concurrency, total, &mix, seed);
        prop_assert_eq!(schedule.offered() as u64, total);
        prop_assert_eq!(schedule.concurrency, concurrency);
    }

    /// A drawn shape is always one of the declared shapes, for every value in
    /// the unit interval.
    #[test]
    fn a_draw_always_lands_on_a_declared_shape(mix in mix_strategy(), value in 0.0_f64..1.0) {
        prop_assert!(mix.0.contains_key(mix.draw(value)));
    }

    /// The generator only ever produces values inside the unit interval, which
    /// is what makes the draw total.
    #[test]
    fn the_generator_stays_in_the_unit_interval(seed in any::<u64>()) {
        let mut rng = Rng::seeded(seed);
        for _ in 0..256 {
            let value = rng.next_unit();
            prop_assert!((0.0..1.0).contains(&value), "{}", value);
        }
    }

    /// Percentiles are ordered and every one of them is an observed sample.
    #[test]
    fn percentiles_are_ordered_and_drawn_from_the_sample(
        values in prop::collection::vec(0.0_f64..1_000_000.0, 1..500),
    ) {
        let mut recorder = Recorder::new();
        for value in &values {
            recorder.record(Metric::EndToEndMs, *value);
        }
        let summary = recorder.summary(Metric::EndToEndMs).expect("samples exist");
        prop_assert_eq!(summary.count, values.len());
        prop_assert!(summary.min <= summary.p50);
        prop_assert!(summary.p50 <= summary.p95);
        prop_assert!(summary.p95 <= summary.p99);
        prop_assert!(summary.p99 <= summary.max);
        for percentile in [summary.p50, summary.p95, summary.p99] {
            prop_assert!(values.iter().any(|value| (value - percentile).abs() < f64::EPSILON));
        }
        prop_assert!(summary.mean >= summary.min && summary.mean <= summary.max);
    }

    /// A campaign passes only when every declared gate reported and no blocking
    /// budget was missed.
    #[test]
    fn a_campaign_passes_only_when_every_gate_reported_and_none_was_missed(
        reported in prop::collection::vec(any::<bool>(), 1..8),
        met in prop::collection::vec(any::<bool>(), 8),
    ) {
        let declared: Vec<Gate> = (0..reported.len())
            .map(|index| Gate {
                id: format!("LOAD-{index}"),
                kind: GateKind::Budget,
                blocking: true,
                assert: "observed <= budget".to_owned(),
                source: "this property".to_owned(),
            })
            .collect();
        let mut report = GateReport::new();
        for (index, gate) in declared.iter().enumerate() {
            if !reported[index] {
                continue;
            }
            let verdict = if met.get(index).copied().unwrap_or(true) {
                GateVerdict::Met
            } else {
                GateVerdict::Unmet
            };
            report.push(GateOutcome::record(gate, verdict, "observed 1 of 1"));
        }
        let all_reported = reported.iter().all(|flag| *flag);
        let any_missed = declared
            .iter()
            .enumerate()
            .any(|(index, _)| reported[index] && !met.get(index).copied().unwrap_or(true));
        prop_assert_eq!(report.passed(&declared), all_reported && !any_missed);
        for failure in report.failures() {
            prop_assert!(failure.starts_with("[perf-gate-unmet] gate LOAD-"), "{}", failure);
            prop_assert!(failure.contains("(source: this property)"), "{}", failure);
        }
    }
}
