//! The control runtime: health, readiness and the metric publisher, on their own OS thread.
//!
//! This is BC-20, and it is a measurement rather than a preference. The activation-pool
//! spike found synchronous CPU on the main reactor delaying `/health` past the load
//! balancer's threshold, and ECS replaced tasks that were working perfectly — taking their
//! in-flight non-replayable effects with them. A health responder that shares a scheduler
//! with the work it reports on cannot answer while that work is saturating the scheduler.
//!
//! So the responder gets a dedicated OS thread running its own `new_current_thread` runtime.
//! It does no work beyond reading two atomics and writing a short response, and nothing the
//! main runtime does can delay it.

use crate::health::{HealthRoute, LivenessInputs, Probe, ReadinessInputs, liveness, readiness};
use crate::pressure::PressureState;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};

/// The state the health responder reads.
///
/// Atomics rather than a lock, because the whole point is that answering a probe cannot
/// block on anything the main runtime holds. A lock here would reintroduce exactly the
/// coupling the dedicated thread exists to remove.
#[derive(Debug, Default)]
pub struct HealthState {
    supervisors_intact: AtomicBool,
    control_responsive: AtomicBool,
    /// The last rolling summary the sampler published — a summary, never the raw ring and
    /// never the latest sample. Sequence-stamped so the responder reads a whole one without
    /// waiting on anything the main runtime holds.
    reactor: crate::reactor::PublishedSummary,
    reactor_delay_bound_ms: AtomicU32,
    bindings_validated: AtomicBool,
    catalog_verified: AtomicBool,
    store_reachable: AtomicBool,
    schema_matched: AtomicBool,
    draining: AtomicBool,
    active_activations: AtomicU32,
    safety_cap: AtomicU32,
    /// The last state the pressure sampler published, as its declared code.
    ///
    /// A copy of the gate rather than the gate itself, so the responder reads only atomics it
    /// owns — the same reason the reactor's active-activation count is published here rather
    /// than pulled from admission. The sampler republishes on every sample, so the copy cannot
    /// outlive the measurement by more than one interval.
    pressure_state: AtomicU8,
    probes_served: AtomicU64,
}

impl HealthState {
    /// A process that has started but validated nothing.
    ///
    /// Readiness starts false and is never defaulted true: a process that reported ready
    /// before validating its bindings would admit work it cannot serve.
    #[must_use]
    pub fn starting(reactor_delay_bound_ms: u32, safety_cap: u32) -> Arc<Self> {
        let state = Self::default();
        state.supervisors_intact.store(true, Ordering::SeqCst);
        state.control_responsive.store(true, Ordering::SeqCst);
        state
            .reactor_delay_bound_ms
            .store(reactor_delay_bound_ms, Ordering::SeqCst);
        state.safety_cap.store(safety_cap, Ordering::SeqCst);
        Arc::new(state)
    }

    /// Records that configuration and every secret binding validated.
    pub fn bindings_validated(&self) {
        self.bindings_validated.store(true, Ordering::SeqCst);
    }

    /// Records that the pinned catalog loaded and its signature verified.
    pub fn catalog_verified(&self) {
        self.catalog_verified.store(true, Ordering::SeqCst);
    }

    /// Records the store's last probe result.
    pub fn store_reachable(&self, reachable: bool) {
        self.store_reachable.store(reachable, Ordering::SeqCst);
    }

    /// Records that the generated schema hashes match this build.
    pub fn schema_matched(&self) {
        self.schema_matched.store(true, Ordering::SeqCst);
    }

    /// Records that a supervised task died.
    pub fn supervisor_lost(&self) {
        self.supervisors_intact.store(false, Ordering::SeqCst);
    }

    /// Publishes one rolling reactor-delay summary.
    ///
    /// Called once a second by the sampler, whatever the summary says. Maximum and missed
    /// ticks are evidence a guardrail reads even when the rolling p99 is nowhere near its
    /// bound, so publication is unconditional.
    pub fn publish_reactor_summary(&self, summary: &crate::reactor::ReactorSummary) {
        self.reactor.publish(summary);
    }

    /// The last whole rolling summary published, if there is one.
    #[must_use]
    pub fn reactor_summary(&self) -> Option<crate::reactor::ReactorSummary> {
        self.reactor.read()
    }

    /// The bound above which a rolling p99 is a wedge.
    ///
    /// Read by the sampler when it builds its window, so the bound the window compares
    /// against and the bound the refusal names are the same value rather than two constants
    /// that agree today.
    #[must_use]
    pub fn reactor_delay_bound_ms(&self) -> u32 {
        self.reactor_delay_bound_ms.load(Ordering::SeqCst)
    }

    /// Records how many activations are running.
    pub fn observe_active(&self, active: u32) {
        self.active_activations.store(active, Ordering::SeqCst);
    }

    /// Records the memory-pressure state the sampler measured.
    ///
    /// Called on every sample, whatever it says. Readiness must never depend on a diagnostic
    /// emit that a full telemetry queue is entitled to drop.
    pub fn observe_pressure(&self, state: PressureState) {
        self.pressure_state.store(state.code(), Ordering::SeqCst);
    }

    /// The last memory-pressure state published.
    #[must_use]
    pub fn pressure_state(&self) -> PressureState {
        PressureState::from_code(self.pressure_state.load(Ordering::SeqCst))
    }

    /// Starts drain. Idempotent.
    pub fn start_drain(&self) {
        self.draining.store(true, Ordering::SeqCst);
    }

    /// Whether drain has started.
    #[must_use]
    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }

    /// How many probes have been answered.
    #[must_use]
    pub fn probes_served(&self) -> u64 {
        self.probes_served.load(Ordering::SeqCst)
    }

    /// Answers one route.
    #[must_use]
    pub fn probe(&self, route: HealthRoute) -> Probe {
        self.probes_served.fetch_add(1, Ordering::SeqCst);
        match route {
            HealthRoute::Live => liveness(&LivenessInputs {
                supervisors: dependency(self.supervisors_intact.load(Ordering::SeqCst)),
                control_runtime: dependency(self.control_responsive.load(Ordering::SeqCst)),
                reactor: self.reactor.read(),
                reactor_delay_bound_ms: self.reactor_delay_bound_ms.load(Ordering::SeqCst),
            }),
            HealthRoute::Ready => readiness(&ReadinessInputs {
                bindings: dependency(self.bindings_validated.load(Ordering::SeqCst)),
                catalog: dependency(self.catalog_verified.load(Ordering::SeqCst)),
                store: dependency(self.store_reachable.load(Ordering::SeqCst)),
                schema_hashes: dependency(self.schema_matched.load(Ordering::SeqCst)),
                draining: self.draining.load(Ordering::SeqCst),
                active_activations: self.active_activations.load(Ordering::SeqCst),
                safety_cap: self.safety_cap.load(Ordering::SeqCst),
                pressure: self.pressure_state(),
            }),
        }
    }

    /// Answers a raw request line, or reports that the path is not served.
    ///
    /// A path this process does not serve is a 404 rather than a 200, so a stale target
    /// group pointing at a superseded name fails visibly instead of keeping the task in
    /// service on a route that means nothing.
    #[must_use]
    pub fn respond(&self, method: &str, path: &str) -> HttpResponse {
        if method != "GET" {
            return HttpResponse {
                status: 405,
                body: "method not allowed".to_owned(),
            };
        }
        HealthRoute::from_path(path).map_or_else(
            || HttpResponse {
                status: 404,
                body: "not found".to_owned(),
            },
            |route| {
                let probe = self.probe(route);
                HttpResponse {
                    status: probe.status,
                    body: probe.body(),
                }
            },
        )
    }
}

const fn dependency(value: bool) -> crate::health::Dependency {
    if value {
        crate::health::Dependency::Satisfied
    } else {
        crate::health::Dependency::Unsatisfied
    }
}

/// One health response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// The status code.
    pub status: u16,
    /// The body.
    pub body: String,
}

impl HttpResponse {
    /// The response as bytes on the wire.
    ///
    /// Written by hand rather than through the shared HTTP composition: this responder must
    /// share nothing with the request path it reports on, and a router pulled in here would
    /// be one more thing that can be slow when the process is under the load the probe
    /// exists to describe.
    #[must_use]
    pub fn render(&self) -> String {
        format!(
            "HTTP/1.1 {} {}\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            self.status,
            reason(self.status),
            self.body.len(),
            self.body
        )
    }
}

const fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Service Unavailable",
    }
}

/// Parses the request line of an HTTP/1.1 request.
///
/// Bounded and total: anything that is not a plain `METHOD SP PATH SP VERSION` line is
/// `None`, and the caller answers 400. The responder never allocates per header.
#[must_use]
pub fn parse_request_line(request: &str) -> Option<(&str, &str)> {
    let line = request.lines().next()?;
    let mut parts = line.split(' ');
    let method = parts.next()?;
    let target = parts.next()?;
    let version = parts.next()?;
    if !version.starts_with("HTTP/") {
        return None;
    }
    // A query string is not part of the route: `/internal/readyz?x=1` is the readiness
    // probe, and treating it as an unknown path would take the task out of service.
    Some((method, target.split('?').next().unwrap_or(target)))
}

#[cfg(test)]
mod tests {
    use super::{HealthState, parse_request_line};
    use crate::health::{LIVE_PATH, READY_PATH};
    use crate::pressure::PressureState;
    use crate::reactor::{ReactorSummary, SUSTAINED_BREACH_SUMMARIES};

    fn state() -> std::sync::Arc<HealthState> {
        HealthState::starting(50, 200)
    }

    /// A full window whose rolling p99 is over the bound for `consecutive_breaches` summaries.
    fn breaching(consecutive_breaches: u32) -> ReactorSummary {
        ReactorSummary {
            p50_ms: 1,
            p95_ms: 40,
            rolling_p99_ms: 51,
            maximum_ms: 51,
            samples: 300,
            missed_ticks: 0,
            window_age_ms: 29_900,
            consecutive_breaches,
        }
    }

    fn ready(state: &HealthState) {
        state.bindings_validated();
        state.catalog_verified();
        state.store_reachable(true);
        state.schema_matched();
    }

    /// Readiness starts false. A process that reported ready before validating its bindings
    /// would admit work it cannot serve, which is worse than being replaced.
    #[test]
    fn a_starting_process_is_live_but_never_ready() {
        let state = state();
        assert_eq!(state.respond("GET", LIVE_PATH).status, 200);
        assert_eq!(state.respond("GET", READY_PATH).status, 503);
    }

    #[test]
    fn a_validated_process_becomes_ready() {
        let state = state();
        ready(&state);
        assert_eq!(state.respond("GET", READY_PATH).status, 200);
        assert_eq!(state.respond("GET", READY_PATH).body, "ok");
    }

    /// Drain fails readiness while liveness still passes. Reversing this would have the
    /// orchestrator kill a draining task along with the effects it is trying to finish.
    #[test]
    fn drain_fails_readiness_and_leaves_liveness_alone() {
        let state = state();
        ready(&state);
        state.start_drain();
        assert!(state.is_draining());
        assert_eq!(state.respond("GET", READY_PATH).status, 503);
        assert_eq!(state.respond("GET", LIVE_PATH).status, 200);
    }

    #[test]
    fn a_lost_supervisor_fails_liveness() {
        let state = state();
        ready(&state);
        state.supervisor_lost();
        assert_eq!(state.respond("GET", LIVE_PATH).status, 503);
    }

    #[test]
    fn the_safety_cap_fails_readiness_only() {
        let state = state();
        ready(&state);
        state.observe_active(200);
        assert_eq!(state.respond("GET", READY_PATH).status, 503);
        assert_eq!(state.respond("GET", LIVE_PATH).status, 200);
    }

    /// Measured pressure takes the task out of the load balancer's rotation and puts it back
    /// when the measurement recovers. Liveness never moves: killing a task for holding memory
    /// would take the effects it is settling with it.
    #[test]
    fn memory_pressure_fails_readiness_only_and_recovers() {
        let state = state();
        ready(&state);
        state.observe_pressure(PressureState::Warning);
        assert_eq!(state.respond("GET", READY_PATH).status, 200);

        state.observe_pressure(PressureState::AdmissionStop);
        assert_eq!(state.respond("GET", READY_PATH).status, 503);
        assert_eq!(state.respond("GET", LIVE_PATH).status, 200);
        assert!(
            state
                .respond("GET", READY_PATH)
                .body
                .contains("memory pressure admission-stop")
        );

        state.observe_pressure(PressureState::Critical);
        assert_eq!(state.respond("GET", READY_PATH).status, 503);
        assert_eq!(state.respond("GET", LIVE_PATH).status, 200);

        state.observe_pressure(PressureState::Normal);
        assert_eq!(state.respond("GET", READY_PATH).status, 200);
    }

    /// Liveness moves on published summaries, never on one sample. A task with no summary
    /// yet is live; one breaching summary is not enough; two are; a recovered summary
    /// restores it.
    #[test]
    fn liveness_follows_the_published_window_rather_than_the_latest_sample() {
        let state = state();
        ready(&state);
        assert_eq!(state.reactor_summary(), None);
        assert_eq!(state.respond("GET", LIVE_PATH).status, 200);

        state.publish_reactor_summary(&breaching(1));
        assert_eq!(state.respond("GET", LIVE_PATH).status, 200);

        state.publish_reactor_summary(&breaching(SUSTAINED_BREACH_SUMMARIES));
        assert_eq!(state.respond("GET", LIVE_PATH).status, 503);

        state.publish_reactor_summary(&ReactorSummary {
            rolling_p99_ms: 50,
            consecutive_breaches: 0,
            ..breaching(0)
        });
        assert_eq!(state.respond("GET", LIVE_PATH).status, 200);
    }

    /// A guardrail reads the worst case and the ticks the reactor never gave the sampler,
    /// and it has to be able to read them while everything is healthy.
    #[test]
    fn a_healthy_window_is_still_published_in_full() {
        let state = state();
        ready(&state);
        state.publish_reactor_summary(&ReactorSummary {
            rolling_p99_ms: 4,
            maximum_ms: 900,
            missed_ticks: 9,
            consecutive_breaches: 0,
            ..breaching(0)
        });
        let summary = state.reactor_summary().expect("a summary was published");
        assert_eq!(state.respond("GET", LIVE_PATH).status, 200);
        assert_eq!(summary.maximum_ms, 900);
        assert_eq!(summary.missed_ticks, 9);
        assert_eq!(summary.samples, 300);
        assert_eq!(summary.window_age_ms, 29_900);
    }

    /// A stale target group pointing at a superseded path must fail visibly rather than
    /// keeping the task in service on a route that means nothing.
    #[test]
    fn a_superseded_path_is_a_404_rather_than_a_200() {
        let state = state();
        ready(&state);
        for path in ["/livez", "/readyz", "/healthz", "/internal/livez"] {
            assert_eq!(state.respond("GET", path).status, 404, "{path}");
        }
    }

    #[test]
    fn only_get_is_answered() {
        let state = state();
        assert_eq!(state.respond("POST", LIVE_PATH).status, 405);
    }

    /// A query string is not part of the route. Treating `?x=1` as an unknown path would
    /// take the task out of service for a probe that is asking the right question.
    #[test]
    fn a_query_string_does_not_change_the_route() {
        assert_eq!(
            parse_request_line("GET /internal/readyz?x=1 HTTP/1.1\r\nhost: a\r\n\r\n"),
            Some(("GET", READY_PATH))
        );
    }

    #[test]
    fn a_malformed_request_line_is_refused_rather_than_guessed() {
        assert_eq!(parse_request_line(""), None);
        assert_eq!(parse_request_line("GET"), None);
        assert_eq!(parse_request_line("GET /internal/healthz"), None);
        assert_eq!(parse_request_line("GET /internal/healthz NOTHTTP"), None);
    }

    #[test]
    fn a_rendered_response_carries_its_own_length_and_closes() {
        let state = state();
        let rendered = state.respond("GET", LIVE_PATH).render();
        assert!(rendered.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(rendered.contains("content-length: 2\r\n"));
        assert!(rendered.contains("connection: close\r\n"));
        assert!(rendered.ends_with("\r\n\r\nok"));
    }

    #[test]
    fn every_answered_probe_is_counted() {
        let state = state();
        assert_eq!(state.probes_served(), 0);
        let _ = state.respond("GET", LIVE_PATH);
        let _ = state.respond("GET", READY_PATH);
        let _ = state.respond("GET", "/nope");
        assert_eq!(state.probes_served(), 2, "an unknown path is not a probe");
    }
}
