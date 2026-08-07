//! The started-artifact evidence for `brain-mux`.
//!
//! A live companion exists to carry exactly this, so the shape is fixed: bind to a deployed
//! plane through `aex_test_harness::required_env!`, which panics with the variable's name
//! when the binding is absent. An absent plane is a failure, never a skip — a smoke lane
//! that reported green because it could not reach anything would be worse than no lane.
//!
//! What is asserted without a plane is the part that is a property of this build rather than
//! of a deployment: the health contract the load balancer targets, and the release row that
//! declares it. Renaming one of those paths silently takes every task out of service, and
//! that is worth catching in a lane that runs everywhere rather than only where a plane
//! exists.
//!
//! # Which of the §8.1 pass criteria this lane discharges
//!
//! A test executor holds one thing: the endpoint of a task somebody else started. Everything
//! provable from there is proved here; nothing else is simulated, because a simulated drain
//! is evidence about the simulation.
//!
//! Discharged:
//!
//! - **declared port** — the endpoint this lane probes is the port `release/units.toml`
//!   declares, read out of that file rather than written down, and a task answers on it;
//! - **health** — `/internal/healthz` answers 200;
//! - **readiness reaches 200 after probes** — `/internal/readyz` is polled to a bounded
//!   deadline, and every answer before the first 200 has to be a 503 that names the
//!   dependency it is waiting on;
//! - **the superseded paths do not resolve** — a stale target group pointing at one of them
//!   would otherwise keep a task in service on a route that means nothing.
//!
//! Owed to a plane-side campaign, because each needs control over the deployment that a test
//! executor does not have:
//!
//! - **exact OCI artifact** — no health response names an image digest; the deployment
//!   record does;
//! - **dependency loss and recovery**, and with it "readiness fails before dependency loss
//!   can admit new work" — proving it means taking `DynamoDB` or `SQS` away from a running
//!   task and watching readiness follow;
//! - **two-task replacement and the absence of a target-registration loop** — a target group
//!   and a service event stream, not an HTTP response;
//! - **graceful drain and no non-replayable effect loss** — stopping a task mid-flight and
//!   accounting for what it owed.
//!
//! A green run here is evidence that one task started, answers, and answers where the
//! release says it will. It is not evidence that §8.1 passes.

use aex_test_harness::{Lane, TestRun};
use core::time::Duration;
use reqwest::{Client, StatusCode, Url};
use std::time::Instant;

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

/// The release registry, which is the one place the task's port and probe paths are declared.
///
/// Read at compile time out of the workspace rather than at run time out of a working
/// directory, so the port this lane asserts is the port the release plan carries no matter
/// where an executor is invoked from. The alternative — writing `8080` down here — is a
/// second declaration of the one number a binary and a target group have to agree on, which
/// is the drift the generated task shape exists to close.
const UNITS: &str = include_str!("../../../../release/units.toml");

/// The release unit this companion is the evidence for.
const UNIT_ID: &str = "brain-mux";

/// How long a connection may take to establish.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a whole request may take.
///
/// Bounded together with the connect deadline because a probe that hangs is a lane that
/// never reports: a black-holed address has to fail in seconds and name itself, rather than
/// wait on whatever the kernel would eventually do.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// How long readiness has to reach 200.
///
/// The task's dependency probe proves its store and its wake queue reachable before its
/// first sleep and re-proves them on a ten-second cadence, so a task that is going to become
/// ready has done so well inside this window. Bounded rather than open-ended for the same
/// reason as the request deadline.
const READY_DEADLINE: Duration = Duration::from_secs(30);

/// How long to wait between readiness attempts.
const READY_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Every reason readiness gives, exactly as the deployable renders it.
///
/// The rendered text is the contract: it is what an operator reads off a 503 and what this
/// lane asserts the shape of, rather than reaching into a type that belongs to the binary.
/// Held as fragments so a reason that carries numbers — the safety cap names its counts —
/// still matches. A new reason has to be added here, and until it is, a 503 nobody can
/// explain fails this lane instead of passing it.
const READINESS_REASONS: [&str; 6] = [
    "configuration bindings not validated",
    "catalog absent or unverified",
    "store unreachable",
    "schema hashes do not match this build",
    "draining",
    "safety cap",
];

/// The `[[unit]]` block the release registry gives this deployable.
///
/// # Panics
///
/// Panics when the registry declares the unit zero times or more than once. Either is a
/// registry defect, and inventing a port for a unit nobody declared would make this lane
/// probe a number no deployment uses.
fn unit_row() -> &'static str {
    let id = format!("id = \"{UNIT_ID}\"");
    let mut rows = UNITS
        .split("[[unit]]")
        .filter(|row| row.lines().any(|line| line.trim() == id.as_str()));
    let row = rows
        .next()
        .unwrap_or_else(|| panic!("release/units.toml declares no `{UNIT_ID}` unit"));
    assert!(
        rows.next().is_none(),
        "release/units.toml declares `{UNIT_ID}` more than once, so there is no one shape to \
         probe"
    );
    row
}

/// The value the release row gives `key`, which it must give exactly once.
fn declared(key: &str) -> &'static str {
    let prefix = format!("{key} = ");
    let mut values = unit_row()
        .lines()
        .filter_map(|line| line.trim().strip_prefix(prefix.as_str()));
    let value = values
        .next()
        .unwrap_or_else(|| panic!("the `{UNIT_ID}` release row declares no `{key}`"));
    assert!(
        values.next().is_none(),
        "the `{UNIT_ID}` release row declares `{key}` more than once"
    );
    value.trim()
}

/// The TCP port the release row declares.
fn declared_port() -> u16 {
    let value = declared("port");
    value.parse().unwrap_or_else(|error| {
        panic!("the `{UNIT_ID}` release row declares port `{value}`, which is not a port: {error}")
    })
}

/// A quoted string the release row declares, with its quotes removed.
fn declared_path(key: &str) -> &'static str {
    let value = declared(key);
    value
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or_else(|| {
            panic!("`{key}` in the `{UNIT_ID}` release row is `{value}`, not a quoted path")
        })
}

/// A transport failure together with everything under it.
///
/// The client renders only `error sending request for url (...)` at the top level. Whether
/// that was a refused connection, a name that does not resolve or the request deadline is in
/// the source chain, and those are three different operator actions, so the chain is
/// flattened into the failure rather than left for someone to guess at.
fn causes(error: &(dyn std::error::Error + 'static)) -> String {
    let mut rendered = vec![error.to_string()];
    let mut source = error.source();
    while let Some(cause) = source {
        rendered.push(cause.to_string());
        source = cause.source();
    }
    rendered.join(": ")
}

/// A deployed task, and the run identity its failures are reported under.
struct LiveTarget {
    plane: String,
    endpoint: String,
    run: TestRun,
    client: Client,
}

impl LiveTarget {
    /// Binds to the deployed task.
    ///
    /// Reads both bindings before building anything, so a partially configured lane fails
    /// naming the variable it is missing rather than timing out against a default.
    fn bind() -> Self {
        let plane = aex_test_harness::required_env!(PLANE_VAR);
        let endpoint = aex_test_harness::required_env!(ENDPOINT_VAR);
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("the bounded live client must build");
        Self {
            plane,
            endpoint: endpoint.trim_end_matches('/').to_owned(),
            run: TestRun::mint(Lane::Smoke, "brain-core", 0),
            client,
        }
    }

    /// The port the bound endpoint addresses.
    ///
    /// # Panics
    ///
    /// Panics when the binding is not a URL, or is a URL of a scheme with no port and none
    /// written down. A lane that could not say which port it was probing could not assert
    /// anything about the declared one.
    fn endpoint_port(&self) -> u16 {
        let endpoint = &self.endpoint;
        let url = Url::parse(endpoint).unwrap_or_else(|error| {
            panic!("`{ENDPOINT_VAR}` is `{endpoint}`, which is not a URL: {error}")
        });
        url.port_or_known_default()
            .unwrap_or_else(|| panic!("`{ENDPOINT_VAR}` is `{endpoint}`, which addresses no port"))
    }

    /// Performs one bounded `GET`, failing with the address it could not reach.
    async fn get(&self, path: &str) -> (StatusCode, String) {
        let run = self.run.id();
        let plane = &self.plane;
        let url = format!("{}{path}", self.endpoint);
        let response = self.client.get(&url).send().await.unwrap_or_else(|error| {
            panic!(
                "run {run} could not reach {url} on plane `{plane}`: {}",
                causes(&error)
            )
        });
        let status = response.status();
        let body = response.text().await.unwrap_or_else(|error| {
            panic!(
                "run {run} got {status} from {url} on plane `{plane}` and could not read its \
                 body: {}",
                causes(&error)
            )
        });
        (status, body)
    }
}

#[test]
fn the_health_contract_is_the_one_the_load_balancer_targets() {
    assert_eq!(LIVE_PATH, "/internal/healthz");
    assert_eq!(READY_PATH, "/internal/readyz");
    for path in SUPERSEDED {
        assert_ne!(path, LIVE_PATH);
        assert_ne!(path, READY_PATH);
    }
}

/// The registry row is what the deployment is built from, so a lane probing paths or a port
/// it does not declare is probing something nobody deployed. Asserted without a plane, which
/// is also what keeps the port reader itself covered by the lane that runs everywhere.
#[test]
fn the_release_row_declares_the_paths_and_the_port_this_lane_probes() {
    assert_eq!(declared_path("health_path"), LIVE_PATH);
    assert_eq!(declared_path("ready_path"), READY_PATH);
    assert!(
        declared_port() > 0,
        "port 0 asks the kernel to choose, which no target group can probe"
    );
}

/// Liveness asks only whether the process can still make progress, so a started task answers
/// it whatever its dependencies are doing. The port is asserted in the same case because the
/// two claims are one claim: the task answers, and it answers where the release says it will.
#[tokio::test]
async fn a_started_task_answers_liveness_on_the_port_the_release_row_declares() {
    let target = LiveTarget::bind();
    let plane = &target.plane;
    let endpoint = &target.endpoint;
    let declared = declared_port();
    let probed = target.endpoint_port();
    assert_eq!(
        probed, declared,
        "`{ENDPOINT_VAR}` is `{endpoint}` on plane `{plane}`, which probes port {probed}; the \
         release row declares {declared}"
    );

    let (status, body) = target.get(LIVE_PATH).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{endpoint}{LIVE_PATH} on plane `{plane}` answered {status}: {body}"
    );
}

/// Readiness starts false and only becomes true once the task has proved its dependencies
/// with real requests, so this waits rather than sampling once. Every answer before the
/// first 200 has to name what it is waiting on: a 503 that says nothing is a readiness
/// failure an operator cannot act on, and this lane is the thing that would notice.
#[tokio::test]
async fn a_started_task_reaches_readiness_and_names_what_it_waits_on_until_it_does() {
    let target = LiveTarget::bind();
    let plane = &target.plane;
    let endpoint = &target.endpoint;
    let deadline = Instant::now() + READY_DEADLINE;

    let unready = loop {
        let (status, body) = target.get(READY_PATH).await;
        if status == StatusCode::OK {
            assert_eq!(
                body.trim(),
                "ok",
                "{endpoint}{READY_PATH} on plane `{plane}` answered 200 with a body that is not \
                 the ready body"
            );
            return;
        }
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{endpoint}{READY_PATH} on plane `{plane}` answered {status}, which is neither ready \
             nor unready: {body}"
        );
        assert!(
            !body.trim().is_empty(),
            "{endpoint}{READY_PATH} on plane `{plane}` answered 503 without saying what is \
             unsatisfied"
        );
        for reason in body.lines().filter(|line| !line.trim().is_empty()) {
            assert!(
                READINESS_REASONS.iter().any(|known| reason.contains(known)),
                "{endpoint}{READY_PATH} on plane `{plane}` is unready for `{reason}`, which names \
                 no dependency this contract knows"
            );
        }
        if Instant::now() >= deadline {
            break body;
        }
        tokio::time::sleep(READY_POLL_INTERVAL).await;
    };

    panic!(
        "{endpoint}{READY_PATH} on plane `{plane}` never reached 200 within {} s; it is still \
         unready for: {unready}",
        READY_DEADLINE.as_secs()
    );
}

/// A stale target group aimed at a superseded name would keep reporting a task healthy on a
/// route that means nothing, and the misconfiguration would surface only on the deploy that
/// finally removed the old one.
#[tokio::test]
async fn a_started_task_serves_none_of_the_superseded_health_paths() {
    let target = LiveTarget::bind();
    let plane = &target.plane;
    let endpoint = &target.endpoint;
    for path in SUPERSEDED {
        let (status, body) = target.get(path).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{endpoint}{path} on plane `{plane}` answered {status} instead of 404, so a target \
             group pointed at it would still register: {body}"
        );
    }
}
