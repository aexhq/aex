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
//!
//! # The cleanup ledger
//!
//! `[role.live_companion]` says a live companion owns the cleanup-ledger flush. This lane
//! minted a `TestRun` — and with it a `CleanupLedger` — and then never installed a
//! reclamation path, never flushed, and never asked what was outstanding. A ledger nobody
//! reads is a ledger that cannot report anything.
//!
//! What this lane can honestly install is [`NoReclamationRoute`], which **refuses**. A test
//! executor here holds one thing: the endpoint of a task somebody else started. It has no
//! route to terminate a generation, stop a task or delete a record, so a reclaimer that
//! answered `Ok(())` would mark entries released having deleted nothing — the same
//! green-no-op the release tool's own `UnavailableAdapter` exists to refuse. Recording
//! something this lane cannot release therefore fails loudly and names the steps it did not
//! take, rather than passing and leaving the resource to the out-of-band janitor.
//!
//! [`LiveTarget::finish`] writes the ledger to the run's artifact directory and asserts it
//! holds nothing outstanding. The `Drop` on `CleanupLedger` is the backstop for a case that
//! panics before reaching it.

use aex_test_harness::{Entry, Lane, ReclaimError, Reclaimer, TestRun};
use core::time::Duration;
use reqwest::{Client, StatusCode, Url};
use std::sync::Arc;
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

/// The reclamation path a health probe actually has, which is none.
///
/// It refuses with the steps it did not take rather than reporting a green no-op, which is
/// the same rule every unreachable reclamation in this workspace follows: a lane that is red
/// for a reason is better than one that reports success having done nothing. A stream that
/// needs to record a resource here has to bring a route that can release it.
#[derive(Debug, Default)]
struct NoReclamationRoute;

impl Reclaimer for NoReclamationRoute {
    fn reclaim(&self, entry: &Entry) -> Result<(), ReclaimError> {
        Err(ReclaimError::new(
            entry,
            format!(
                "the brain-mux smoke lane holds only an HTTP endpoint and has no route to \
                 reclaim a `{}`; required steps: {}",
                entry.kind,
                entry.kind.policy().reclaim
            ),
        ))
    }
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
        let run = TestRun::mint(Lane::Smoke, "brain-core", 0);
        assert!(
            run.ledger().install_reclaimer(Arc::new(NoReclamationRoute)),
            "the reclamation path must be installed exactly once"
        );
        Self {
            plane,
            endpoint: endpoint.trim_end_matches('/').to_owned(),
            run,
            client,
        }
    }

    /// Writes the ledger and asserts the run left nothing behind.
    ///
    /// Called at the end of every case that binds a plane. A smoke probe creates nothing, so
    /// this passes with an empty ledger today; it is here so that the first case to record
    /// something is gated the moment it does, rather than after a sweep finds the residue on
    /// a plane.
    fn finish(&self) {
        let path = self
            .run
            .ledger()
            .flush()
            .expect("the cleanup ledger must be written to the run's artifact directory");
        assert!(
            path.0.starts_with(self.run.artifact_dir()),
            "the ledger must land in the run's artifact directory, which is what a lane collects"
        );
        let residue = self.run.ledger().residue();
        assert!(residue.is_empty(), "{}", self.run.ledger().residue_report());
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

/// The reclaimer this lane installs must refuse, and must say what it did not do.
///
/// Asserted without a plane because it is a property of the lane rather than of a
/// deployment. An `Ok(())` here would mark an entry released having deleted nothing, which
/// is worse than the leak it would be hiding: the ledger would report a clean run and the
/// janitor would find the resource by tag hours later with nobody expecting it.
#[test]
fn the_installed_reclaimer_refuses_and_names_the_steps_it_did_not_take() {
    let entry = Entry::new(
        aex_test_harness::ResourceKind::HandsGeneration,
        "generation-0001",
        aex_test_harness::Terminal::Deleted,
        aex_test_harness::TestCaseId("reclaimer-contract".to_owned()),
    );
    let error = Reclaimer::reclaim(&NoReclamationRoute, &entry)
        .expect_err("a health probe has no route to terminate a generation");
    let rendered = error.to_string();
    assert!(rendered.contains("no route to reclaim"), "{rendered}");
    assert!(
        rendered.contains("terminate the generation"),
        "the refusal must carry the policy's own reclamation steps: {rendered}"
    );
}

/// Recording something this lane cannot release has to fail, not pass quietly.
#[test]
fn an_unreleasable_entry_is_residue_rather_than_a_silently_released_one() {
    let run = TestRun::mint(Lane::Smoke, "brain-core", 0);
    assert!(run.ledger().install_reclaimer(Arc::new(NoReclamationRoute)));
    run.ledger().record(Entry::new(
        aex_test_harness::ResourceKind::EcsTask,
        run.resource_name("task"),
        aex_test_harness::Terminal::Deleted,
        aex_test_harness::TestCaseId("reclaimer-contract".to_owned()),
    ));

    let summary = run.ledger().reclaim_all();
    assert!(
        !summary.is_complete(),
        "the refusal must be reported, never absorbed"
    );
    assert_eq!(run.ledger().residue().len(), 1);
    // Taken so the ledger's own Drop does not abort this process; the assertion
    // above is what makes the refusal a failure.
    let _ = run.ledger().take_residue_report();
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

    target.finish();
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
            target.finish();
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

    target.finish();
}
