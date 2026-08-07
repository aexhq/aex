//! The rolling reactor-delay window.
//!
//! The sampler observes one scheduling-lateness value per tick. Storing that value under the
//! name `p99` — which is what this module replaces — made liveness a coin flip: one punctual
//! tick erased a thirty-second stall, one late tick fabricated a breach, and the load
//! balancer could take a working task out of service on a single sample. A value that is
//! overwritten every tick is an instantaneous reading, and calling it a percentile did not
//! make it one.
//!
//! A percentile is a property of a set, so this keeps the set: a fixed 300-sample ring —
//! thirty seconds at the sampler's tick — summarised once a second by copying and sorting it.
//! Three hundred `u32`s is 1.2 KiB, and one sort per second is cheaper than the machinery a
//! streaming histogram would need in order to avoid it.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

/// The most samples the window retains.
///
/// At the sampler's 100 ms tick this is thirty seconds, which is the span a wedge has to
/// persist over before it is worth replacing a task that still owns non-replayable effects.
pub const WINDOW_SAMPLES: usize = 300;

/// How often the window is summarised and published.
pub const SUMMARY_INTERVAL: core::time::Duration = core::time::Duration::from_secs(1);

/// The fewest retained samples a rolling p99 is evaluated from.
///
/// Below one hundred the nearest-rank p99 *is* the window maximum — at n = 99 the rank lands
/// on the last element — so a single late tick during start-up would read as a breach. One
/// hundred is the smallest set on which one outlier cannot move the 99th percentile, which is
/// the whole property liveness is relying on.
pub const MINIMUM_SAMPLES: u32 = 100;

/// Consecutive published summaries above the bound before the reactor is called wedged.
///
/// One breaching summary is one second of evidence. Requiring two means an allocator pause or
/// a cold page-in cannot get a working task killed, while a genuine wedge is still reported
/// within two seconds of the window filling.
pub const SUSTAINED_BREACH_SUMMARIES: u32 = 2;

/// How many times a reader retries before reporting no summary.
///
/// The publisher's critical section is eight atomic stores once a second, so a reader
/// observing even one concurrent publication is already unlikely and four in a row requires
/// the publisher to be descheduled mid-write four times. Bounding it is what keeps the
/// control thread's read free of any wait on the main runtime.
const READ_ATTEMPTS: usize = 4;

/// One observation, with the tick opportunities lost before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sample {
    at: Instant,
    lateness_ms: u32,
    missed_ticks: u32,
}

/// A rolling summary of reactor scheduling lateness.
///
/// Every field is a property of the retained set, never of the newest observation. The naming
/// is deliberate: nothing here may be read as "the delay right now".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReactorSummary {
    /// Median retained lateness.
    pub p50_ms: u32,
    /// 95th-percentile retained lateness.
    pub p95_ms: u32,
    /// 99th-percentile retained lateness, by nearest rank over the retained set.
    pub rolling_p99_ms: u32,
    /// The worst retained lateness.
    pub maximum_ms: u32,
    /// How many samples the percentiles were computed from.
    pub samples: u32,
    /// Tick opportunities the reactor never gave the sampler, over the retained window.
    pub missed_ticks: u32,
    /// Wall time from the oldest retained sample to the newest.
    pub window_age_ms: u32,
    /// Consecutive published summaries whose rolling p99 exceeded the bound.
    pub consecutive_breaches: u32,
}

impl ReactorSummary {
    /// Whether liveness should report the reactor wedged.
    ///
    /// The minimum-sample rule is enforced where the breach is counted, in
    /// [`ReactorWindow::publish_due`], so this is the single threshold it produces rather than
    /// a second place the policy could drift.
    #[must_use]
    pub const fn is_wedged(&self) -> bool {
        self.consecutive_breaches >= SUSTAINED_BREACH_SUMMARIES
    }
}

/// The fixed-capacity sample ring and the breach policy over it.
#[derive(Debug)]
pub struct ReactorWindow {
    tick: core::time::Duration,
    bound_ms: u32,
    samples: VecDeque<Sample>,
    sorted: Vec<u32>,
    next_publication: Option<Instant>,
    consecutive_breaches: u32,
}

impl ReactorWindow {
    /// A window that samples at `tick` and calls a rolling p99 above `bound_ms` a breach.
    ///
    /// Both are supplied rather than read from the composition, so the ring's own rules can be
    /// asserted on a deterministic clock without standing up a runtime.
    #[must_use]
    pub fn new(tick: core::time::Duration, bound_ms: u32) -> Self {
        Self {
            tick,
            bound_ms,
            // Allocated once, at capacity. Every push past the capacity evicts, so the ring
            // never reallocates and the sampler never allocates on the reactor it measures.
            samples: VecDeque::with_capacity(WINDOW_SAMPLES),
            sorted: Vec::with_capacity(WINDOW_SAMPLES),
            next_publication: None,
            consecutive_breaches: 0,
        }
    }

    /// Records one tick that ran `lateness` after it was due.
    pub fn observe(&mut self, at: Instant, lateness: core::time::Duration) {
        // A tick that runs a whole period late is a period in which the sampler never ran.
        // Counting those is the difference between "the reactor is slow" and "the reactor
        // stopped", which the lateness of the one tick that did run cannot distinguish.
        let tick_ms = self.tick.as_millis().max(1);
        let missed_ticks = u32::try_from(lateness.as_millis() / tick_ms).unwrap_or(u32::MAX);
        let lateness_ms = u32::try_from(lateness.as_millis()).unwrap_or(u32::MAX);
        if self.samples.len() == WINDOW_SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back(Sample {
            at,
            lateness_ms,
            missed_ticks,
        });
        if self.next_publication.is_none() {
            self.next_publication = Some(at + SUMMARY_INTERVAL);
        }
    }

    /// The summary for this second, once a second has passed.
    ///
    /// Returns `None` between publications. The breach counter advances only here, so "two
    /// consecutive breaching summaries" is two seconds of evidence rather than two samples.
    pub fn publish_due(&mut self, now: Instant) -> Option<ReactorSummary> {
        let due = self.next_publication?;
        if now < due {
            return None;
        }
        self.next_publication = Some(now + SUMMARY_INTERVAL);

        self.sorted.clear();
        self.sorted
            .extend(self.samples.iter().map(|sample| sample.lateness_ms));
        self.sorted.sort_unstable();
        let samples = u32::try_from(self.sorted.len()).unwrap_or(u32::MAX);
        let rolling_p99_ms = percentile(&self.sorted, 99);
        if samples >= MINIMUM_SAMPLES && rolling_p99_ms > self.bound_ms {
            self.consecutive_breaches = self.consecutive_breaches.saturating_add(1);
        } else {
            // The documented recovery condition: one published summary whose rolling p99 is at
            // or below the bound clears the breach outright. A decaying counter would keep a
            // recovered task out of service for as long as it was wedged.
            self.consecutive_breaches = 0;
        }
        Some(ReactorSummary {
            p50_ms: percentile(&self.sorted, 50),
            p95_ms: percentile(&self.sorted, 95),
            rolling_p99_ms,
            maximum_ms: self.sorted.last().copied().unwrap_or(0),
            samples,
            missed_ticks: self.samples.iter().fold(0_u32, |total, sample| {
                total.saturating_add(sample.missed_ticks)
            }),
            window_age_ms: self.window_age_ms(),
            consecutive_breaches: self.consecutive_breaches,
        })
    }

    fn window_age_ms(&self) -> u32 {
        match (self.samples.front(), self.samples.back()) {
            (Some(oldest), Some(newest)) => {
                u32::try_from(newest.at.saturating_duration_since(oldest.at).as_millis())
                    .unwrap_or(u32::MAX)
            }
            _ => 0,
        }
    }
}

/// The nearest-rank percentile of an ascending slice.
///
/// Nearest rank rather than interpolation, because the value reported has to be a lateness the
/// reactor actually exhibited rather than one derived between two it did. `rank = ceil(percent
/// × n / 100)`, so the p99 of 300 samples is the 297th smallest and three outliers in three
/// hundred cannot move it.
fn percentile(sorted: &[u32], percent: u32) -> u32 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (usize::try_from(percent).unwrap_or(100) * sorted.len())
        .div_ceil(100)
        .clamp(1, sorted.len());
    sorted[rank - 1]
}

/// The summary the control thread reads, published without a lock.
///
/// The health responder has its own OS thread precisely so nothing the main runtime does can
/// delay it, and a mutex around the summary would hand that guarantee straight back: the
/// publisher can be descheduled while holding it, exactly when the reactor is under the load
/// the probe is asking about.
///
/// So the fields are atomics stamped with a sequence. The publisher raises the sequence to an
/// odd value, writes, and raises it to an even one; a reader that observes the same even
/// sequence before and after its loads holds a whole summary. A reader that does not, retries,
/// and after [`READ_ATTEMPTS`] reports no summary rather than a torn one. Liveness treats no
/// summary as no evidence and passes, which is the only safe direction: an unreadable summary
/// must never be the reason a task is killed.
#[derive(Debug, Default)]
pub struct PublishedSummary {
    sequence: AtomicU64,
    p50_ms: AtomicU32,
    p95_ms: AtomicU32,
    rolling_p99_ms: AtomicU32,
    maximum_ms: AtomicU32,
    samples: AtomicU32,
    missed_ticks: AtomicU32,
    window_age_ms: AtomicU32,
    consecutive_breaches: AtomicU32,
}

impl PublishedSummary {
    /// Replaces the published summary. One publisher only: the reactor sampler.
    pub fn publish(&self, summary: &ReactorSummary) {
        self.sequence.fetch_add(1, Ordering::SeqCst);
        self.p50_ms.store(summary.p50_ms, Ordering::SeqCst);
        self.p95_ms.store(summary.p95_ms, Ordering::SeqCst);
        self.rolling_p99_ms
            .store(summary.rolling_p99_ms, Ordering::SeqCst);
        self.maximum_ms.store(summary.maximum_ms, Ordering::SeqCst);
        self.samples.store(summary.samples, Ordering::SeqCst);
        self.missed_ticks
            .store(summary.missed_ticks, Ordering::SeqCst);
        self.window_age_ms
            .store(summary.window_age_ms, Ordering::SeqCst);
        self.consecutive_breaches
            .store(summary.consecutive_breaches, Ordering::SeqCst);
        self.sequence.fetch_add(1, Ordering::SeqCst);
    }

    /// The last whole summary published, or `None` if none has been or one could not be read
    /// intact.
    #[must_use]
    pub fn read(&self) -> Option<ReactorSummary> {
        for _ in 0..READ_ATTEMPTS {
            let before = self.sequence.load(Ordering::SeqCst);
            if before == 0 {
                return None;
            }
            if !before.is_multiple_of(2) {
                continue;
            }
            let summary = ReactorSummary {
                p50_ms: self.p50_ms.load(Ordering::SeqCst),
                p95_ms: self.p95_ms.load(Ordering::SeqCst),
                rolling_p99_ms: self.rolling_p99_ms.load(Ordering::SeqCst),
                maximum_ms: self.maximum_ms.load(Ordering::SeqCst),
                samples: self.samples.load(Ordering::SeqCst),
                missed_ticks: self.missed_ticks.load(Ordering::SeqCst),
                window_age_ms: self.window_age_ms.load(Ordering::SeqCst),
                consecutive_breaches: self.consecutive_breaches.load(Ordering::SeqCst),
            };
            if self.sequence.load(Ordering::SeqCst) == before {
                return Some(summary);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MINIMUM_SAMPLES, PublishedSummary, ReactorSummary, ReactorWindow, SUMMARY_INTERVAL,
        SUSTAINED_BREACH_SUMMARIES, WINDOW_SAMPLES, percentile,
    };
    use std::time::Instant;

    const TICK: core::time::Duration = crate::compose::REACTOR_TICK;
    const BOUND_MS: u32 = crate::compose::REACTOR_DELAY_BOUND_MS;

    /// A clock the test moves by hand, so every assertion is about the ring rather than about
    /// how fast the machine running the suite happened to be.
    struct Clock {
        origin: Instant,
        elapsed: core::time::Duration,
    }

    impl Clock {
        fn new() -> Self {
            Self {
                origin: Instant::now(),
                elapsed: core::time::Duration::ZERO,
            }
        }

        fn advance(&mut self, by: core::time::Duration) -> Instant {
            self.elapsed += by;
            self.origin + self.elapsed
        }
    }

    /// Observes `count` ticks that each ran `lateness_ms` late, without publishing.
    fn feed(window: &mut ReactorWindow, clock: &mut Clock, count: u32, lateness_ms: u64) {
        for _ in 0..count {
            let at = clock.advance(TICK);
            window.observe(at, core::time::Duration::from_millis(lateness_ms));
        }
    }

    /// Moves the clock one whole summary interval on and takes the summary that falls due.
    fn publish(window: &mut ReactorWindow, clock: &mut Clock) -> ReactorSummary {
        let at = clock.advance(SUMMARY_INTERVAL);
        window
            .publish_due(at)
            .expect("a whole summary interval has passed")
    }

    /// Nearest rank, not interpolation: every reported value is a lateness the reactor
    /// actually exhibited rather than one derived between two it did.
    #[test]
    fn a_percentile_selects_the_rank_it_names() {
        let hundred: Vec<u32> = (1..=100).collect();
        assert_eq!(percentile(&hundred, 50), 50);
        assert_eq!(percentile(&hundred, 95), 95);
        assert_eq!(percentile(&hundred, 99), 99);
        assert_eq!(percentile(&hundred, 100), 100);

        let three_hundred: Vec<u32> = (1..=300).collect();
        assert_eq!(percentile(&three_hundred, 50), 150);
        assert_eq!(percentile(&three_hundred, 95), 285);
        assert_eq!(percentile(&three_hundred, 99), 297);

        assert_eq!(percentile(&[], 99), 0);
        assert_eq!(percentile(&[7], 99), 7);
    }

    /// Why the minimum is exactly one hundred: at ninety-nine samples the nearest-rank p99
    /// lands on the last element, so it is the window maximum under another name.
    #[test]
    fn below_the_minimum_sample_count_a_p99_is_the_maximum() {
        let mut ninety_nine = vec![0_u32; 98];
        ninety_nine.push(1_000);
        assert_eq!(percentile(&ninety_nine, 99), 1_000);

        let mut hundred = vec![0_u32; 99];
        hundred.push(1_000);
        assert_eq!(percentile(&hundred, 99), 0);
        assert_eq!(
            u32::try_from(hundred.len()).expect("small"),
            MINIMUM_SAMPLES
        );
    }

    /// Thirty seconds of samples, then eviction. A window that grew without bound would keep
    /// reporting a wedge that ended minutes ago.
    #[test]
    fn the_ring_evicts_after_thirty_seconds_of_samples() {
        assert_eq!(
            TICK * u32::try_from(WINDOW_SAMPLES).expect("small"),
            core::time::Duration::from_secs(30),
            "the capacity is thirty seconds at the sampler's own tick"
        );
        let mut window = ReactorWindow::new(TICK, BOUND_MS);
        let mut clock = Clock::new();

        feed(&mut window, &mut clock, 300, 1);
        let full = publish(&mut window, &mut clock);
        assert_eq!(full.samples, 300);
        assert_eq!(full.window_age_ms, 29_900, "299 gaps of one tick");

        feed(&mut window, &mut clock, 300, 9);
        let evicted = publish(&mut window, &mut clock);
        assert_eq!(
            evicted.samples, 300,
            "the ring never grows past its capacity"
        );
        assert_eq!(
            evicted.maximum_ms, 9,
            "nothing from the first thirty seconds survives"
        );
    }

    /// The defect this module replaces: one late tick overwrote the stored value and failed
    /// liveness on its own.
    #[test]
    fn one_isolated_late_tick_never_wedges_the_reactor() {
        let mut window = ReactorWindow::new(TICK, BOUND_MS);
        let mut clock = Clock::new();

        feed(&mut window, &mut clock, 299, 1);
        let at = clock.advance(TICK);
        window.observe(at, core::time::Duration::from_secs(5));

        let summary = publish(&mut window, &mut clock);
        assert_eq!(summary.samples, 300);
        assert_eq!(summary.maximum_ms, 5_000, "the outlier is still reported");
        assert_eq!(
            summary.rolling_p99_ms, 1,
            "one sample in three hundred cannot move the 99th percentile"
        );
        assert_eq!(summary.consecutive_breaches, 0);
        assert!(!summary.is_wedged());
    }

    /// A reactor that is genuinely late has to be reported, and within seconds of the window
    /// carrying enough evidence to say so.
    #[test]
    fn a_sustained_delay_wedges_the_reactor_after_two_consecutive_summaries() {
        let mut window = ReactorWindow::new(TICK, BOUND_MS);
        let mut clock = Clock::new();

        feed(&mut window, &mut clock, 300, 400);
        let first = publish(&mut window, &mut clock);
        assert_eq!(first.samples, 300);
        assert_eq!(first.rolling_p99_ms, 400);
        assert!(first.rolling_p99_ms > BOUND_MS);
        assert_eq!(first.consecutive_breaches, 1);
        assert!(
            !first.is_wedged(),
            "one breaching summary is one second of evidence"
        );

        let second = publish(&mut window, &mut clock);
        assert_eq!(second.consecutive_breaches, SUSTAINED_BREACH_SUMMARIES);
        assert!(second.is_wedged());
    }

    /// A percentile over too few samples is the maximum, so a short window is not evaluated
    /// at all, however late every one of its samples was.
    #[test]
    fn a_window_below_the_minimum_sample_count_is_never_evaluated() {
        let mut window = ReactorWindow::new(TICK, BOUND_MS);
        let mut clock = Clock::new();

        feed(&mut window, &mut clock, 99, 5_000);
        for _ in 0..=SUSTAINED_BREACH_SUMMARIES {
            let summary = publish(&mut window, &mut clock);
            assert_eq!(summary.samples, 99);
            assert!(summary.rolling_p99_ms > BOUND_MS);
            assert_eq!(summary.consecutive_breaches, 0);
            assert!(!summary.is_wedged());
        }
    }

    /// A tick that runs a whole period late is a period in which the sampler never ran, and
    /// the lateness of the one tick that did run cannot say so.
    #[test]
    fn ticks_the_reactor_never_gave_the_sampler_are_counted() {
        let mut window = ReactorWindow::new(TICK, BOUND_MS);
        let mut clock = Clock::new();

        feed(&mut window, &mut clock, 9, 0);
        let at = clock.advance(TICK);
        window.observe(at, core::time::Duration::from_millis(500));

        let summary = publish(&mut window, &mut clock);
        assert_eq!(summary.samples, 10);
        assert_eq!(summary.missed_ticks, 5, "five tick periods of one stall");
    }

    /// Recovery clears outright. A decaying counter would hold a task that is already healthy
    /// out of service for as long as it was wedged.
    #[test]
    fn a_summary_at_or_below_the_bound_clears_a_breach() {
        let mut window = ReactorWindow::new(TICK, BOUND_MS);
        let mut clock = Clock::new();

        feed(&mut window, &mut clock, 300, 400);
        let _ = publish(&mut window, &mut clock);
        assert!(publish(&mut window, &mut clock).is_wedged());

        feed(&mut window, &mut clock, 300, BOUND_MS.into());
        let recovered = publish(&mut window, &mut clock);
        assert_eq!(
            recovered.rolling_p99_ms, BOUND_MS,
            "at the bound is not above it"
        );
        assert_eq!(recovered.consecutive_breaches, 0);
        assert!(!recovered.is_wedged());
    }

    /// Maximum and missed ticks are the evidence a guardrail reads, so they are published on
    /// every summary rather than only on the ones that breach.
    #[test]
    fn a_healthy_summary_still_publishes_its_maximum_and_missed_ticks() {
        let mut window = ReactorWindow::new(TICK, BOUND_MS);
        let mut clock = Clock::new();

        feed(&mut window, &mut clock, 299, 1);
        let at = clock.advance(TICK);
        window.observe(at, core::time::Duration::from_millis(300));

        let summary = publish(&mut window, &mut clock);
        assert!(!summary.is_wedged());
        assert_eq!(summary.maximum_ms, 300);
        assert_eq!(summary.missed_ticks, 3);
        assert_eq!(summary.p50_ms, 1);
        assert_eq!(summary.p95_ms, 1);
    }

    /// Once a second, not once a tick: "two consecutive breaching summaries" has to mean two
    /// seconds of evidence rather than two samples.
    #[test]
    fn a_summary_is_published_once_a_second_rather_than_once_a_tick() {
        assert_eq!(SUMMARY_INTERVAL.as_millis() / TICK.as_millis(), 10);
        let mut window = ReactorWindow::new(TICK, BOUND_MS);
        let mut clock = Clock::new();

        let mut published = 0_u32;
        for _ in 0..30 {
            let at = clock.advance(TICK);
            window.observe(at, core::time::Duration::ZERO);
            if window.publish_due(at).is_some() {
                published += 1;
            }
        }
        assert_eq!(
            published, 2,
            "the first summary falls due one second after the first sample"
        );
    }

    /// A probe that reads no summary must never be a reason to kill a task, so "nothing has
    /// been published yet" is a distinct answer rather than a zeroed one.
    #[test]
    fn an_unpublished_summary_reads_as_no_evidence() {
        let published = PublishedSummary::default();
        assert_eq!(published.read(), None);

        let summary = ReactorSummary {
            rolling_p99_ms: 12,
            maximum_ms: 40,
            samples: 300,
            missed_ticks: 2,
            window_age_ms: 29_900,
            ..ReactorSummary::default()
        };
        published.publish(&summary);
        assert_eq!(published.read(), Some(summary));

        let mut window = ReactorWindow::new(TICK, BOUND_MS);
        assert_eq!(
            window.publish_due(Instant::now()),
            None,
            "a window that has observed nothing has nothing to summarise"
        );
    }

    /// Every field crosses to the control thread together. A reader that could pair one
    /// publication's p99 with another's sample count would report a percentile the window
    /// never computed.
    #[test]
    fn a_published_summary_crosses_whole_or_not_at_all() {
        let published = std::sync::Arc::new(PublishedSummary::default());
        let writer = std::sync::Arc::clone(&published);
        let publishing = std::thread::spawn(move || {
            for round in 1..=5_000_u32 {
                writer.publish(&ReactorSummary {
                    p50_ms: round,
                    p95_ms: round,
                    rolling_p99_ms: round,
                    maximum_ms: round,
                    samples: round,
                    missed_ticks: round,
                    window_age_ms: round,
                    consecutive_breaches: round,
                });
            }
        });
        for _ in 0..50_000 {
            if let Some(summary) = published.read() {
                assert_eq!(summary.p50_ms, summary.rolling_p99_ms);
                assert_eq!(summary.maximum_ms, summary.samples);
                assert_eq!(summary.window_age_ms, summary.consecutive_breaches);
            }
        }
        publishing.join().expect("the publisher finished");
    }
}
