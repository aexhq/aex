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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// The state the health responder reads.
///
/// Atomics rather than a lock, because the whole point is that answering a probe cannot
/// block on anything the main runtime holds. A lock here would reintroduce exactly the
/// coupling the dedicated thread exists to remove.
#[derive(Debug, Default)]
pub struct HealthState {
    supervisors_intact: AtomicBool,
    control_responsive: AtomicBool,
    reactor_delay_p99_ms: AtomicU32,
    reactor_delay_bound_ms: AtomicU32,
    bindings_validated: AtomicBool,
    catalog_verified: AtomicBool,
    store_reachable: AtomicBool,
    schema_matched: AtomicBool,
    draining: AtomicBool,
    active_activations: AtomicU32,
    safety_cap: AtomicU32,
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

    /// Records the reactor's observed lateness.
    pub fn observe_reactor_delay(&self, p99_ms: u32) {
        self.reactor_delay_p99_ms.store(p99_ms, Ordering::SeqCst);
    }

    /// Records how many activations are running.
    pub fn observe_active(&self, active: u32) {
        self.active_activations.store(active, Ordering::SeqCst);
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
                reactor_delay_p99_ms: self.reactor_delay_p99_ms.load(Ordering::SeqCst),
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

    fn state() -> std::sync::Arc<HealthState> {
        HealthState::starting(50, 200)
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

    #[test]
    fn a_wedged_reactor_fails_liveness() {
        let state = state();
        ready(&state);
        state.observe_reactor_delay(51);
        assert_eq!(state.respond("GET", LIVE_PATH).status, 503);
        state.observe_reactor_delay(50);
        assert_eq!(state.respond("GET", LIVE_PATH).status, 200);
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
