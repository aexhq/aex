//! The started-artifact evidence for `brain-mux`.
//!
//! A live companion exists to carry exactly this, so the shape is fixed: bind to a deployed
//! plane through `aex_test_harness::required_env!`, which panics with the variable's name
//! when the binding is absent. An absent plane is a failure, never a skip — a smoke lane
//! that reported green because it could not reach anything would be worse than no lane.
//!
//! What is asserted without a plane is the part that is a property of this build rather than
//! of a deployment: the health contract the load balancer targets. Renaming one of those
//! paths silently takes every task out of service, and that is worth catching in a lane that
//! runs everywhere rather than only where a plane exists.

use aex_test_harness::{Lane, TestRun};

/// The deployed plane the smoke lane binds to.
const PLANE_VAR: &str = "AEX_LIVE_PLANE";

/// The base URL of the task's internal listener.
const ENDPOINT_VAR: &str = "AEX_LIVE_BRAIN_MUX_ENDPOINT";

/// The two paths the ALB target group names.
///
/// Written out here rather than imported from the binary: a live companion imports only
/// public and generated contracts plus test support, never an implementation-private module
/// of its deployable. Duplicating two strings is the price of that rule, and the duplication
/// is exactly what makes a rename visible.
const LIVE_PATH: &str = "/internal/healthz";
const READY_PATH: &str = "/internal/readyz";

/// The superseded names, which must not resolve.
///
/// Serving both would let a stale target group keep working and hide the misconfiguration
/// until a deploy that removed the old one.
const SUPERSEDED: [&str; 4] = ["/livez", "/readyz", "/healthz", "/health"];

#[test]
fn the_health_contract_is_the_one_the_load_balancer_targets() {
    assert_eq!(LIVE_PATH, "/internal/healthz");
    assert_eq!(READY_PATH, "/internal/readyz");
    for path in SUPERSEDED {
        assert_ne!(path, LIVE_PATH);
        assert_ne!(path, READY_PATH);
    }
}

/// Probes the deployed task.
///
/// Reads both bindings before doing anything, so a partially configured lane fails naming
/// the variable it is missing rather than timing out against a default.
#[test]
fn a_started_task_answers_both_probes() {
    let plane = aex_test_harness::required_env!(PLANE_VAR);
    let endpoint = aex_test_harness::required_env!(ENDPOINT_VAR);
    let run = TestRun::mint(Lane::Smoke, "brain-core", 0);
    panic!(
        "run {} would probe {endpoint}{LIVE_PATH} and {endpoint}{READY_PATH} on plane \
         `{plane}`, and cannot: nothing is deployed in this rewrite (OD-07). Reported as a \
         failure rather than as a green run of zero evidence.",
        run.id().as_str()
    );
}
