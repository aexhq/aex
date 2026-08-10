//! The `session-stream-api` load executor.
//!
//! This target is selected by the `load` and `soak` nextest profiles and by no other. The
//! unit lane excludes it by package (`default-filter = 'not package(/^aex-live-/)'`), which
//! is a declared configuration fact rather than a self-skip: nothing here reads an
//! environment variable and returns green.
//!
//! It exists because `LOAD-STREAM-SOCKETS` and `PERF-BASELINE-JOURNAL-FOLD` are
//! **blocking** gates whose descriptors name this package, and until this file existed the
//! package mapped no `load` target at all — so both gates were unrunnable and nothing said
//! so. `aex-workload-unowned` only asked whether a descriptor existed, which it did.
//!
//! Two halves, deliberately separated, following the brain-core executor.
//!
//! 1. **Descriptor conformance** runs with no plane at all. It proves that every blocking
//!    gate the registry assigns this package has a descriptor, that each descriptor parses
//!    and verifies under the same rules the schema declares, and that the two numbers the
//!    gates are actually about — a thousand sockets and a ten-thousand-event fold — are the
//!    numbers the descriptors pin. A campaign whose descriptor is wrong measures the wrong
//!    thing.
//! 2. **Execution** needs a deployed plane. It reads its binding through
//!    `aex_test_harness::required_env!`, which panics with the variable's name when it is
//!    absent — a live prerequisite is a failure, never a skip.
//!
//! Note that the two descriptors sit under **different owner directories**:
//! `regional-services` owns the socket campaign and `regional-domains` owns the fold
//! baseline, while the executor belongs to the package both name as `target`. The loader
//! therefore selects by `target` across the whole descriptor tree rather than by directory.

use aex_load_harness::{GateKind, Phase, WorkloadDescriptor, WorkloadError};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The package every descriptor this executor runs names as its target.
const TARGET: &str = "aex-live-session-stream-api";

/// The whole descriptor tree.
fn workload_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../load/workloads")
        .canonicalize()
        .expect("the workload directory exists")
}

/// The tier profile directory the descriptors' `tier` must resolve against.
fn profile_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../load/profiles")
        .canonicalize()
        .expect("the tier profile directory exists")
}

/// Every `.toml` under a directory tree, sorted, so a run is order-stable.
fn toml_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = std::fs::read_dir(&directory).expect("a readable workload directory");
        for entry in entries {
            let path = entry.expect("a readable directory entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|it| it == "toml") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// The tier ids `tests/load/profiles` declares.
fn known_tiers() -> Vec<String> {
    toml_files(&profile_root())
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path).expect("a readable profile");
            let document: toml::Value = toml::from_str(&text).expect("a profile parses");
            document
                .get("id")
                .and_then(toml::Value::as_str)
                .expect("a profile names its tier")
                .to_owned()
        })
        .collect()
}

/// Every descriptor that names this package as its target, by file stem.
fn descriptors() -> Vec<(String, WorkloadDescriptor)> {
    let mut loaded = Vec::new();
    for path in toml_files(&workload_root()) {
        let text = std::fs::read_to_string(&path).expect("a readable descriptor");
        let name = path
            .file_stem()
            .and_then(|it| it.to_str())
            .expect("a UTF-8 file name")
            .to_owned();
        let descriptor = WorkloadDescriptor::parse(&name, &text)
            .unwrap_or_else(|error: WorkloadError| panic!("{name}: {error}"));
        if descriptor.target == TARGET {
            loaded.push((name, descriptor));
        }
    }
    loaded.sort_by(|left, right| left.0.cmp(&right.0));
    assert!(
        !loaded.is_empty(),
        "`{TARGET}` carries blocking gates, so it owns at least one descriptor"
    );
    loaded
}

/// The gates `release/policy/workload-registry.toml` assigns this package and marks
/// `blocking = true`.
///
/// Hand-written here on purpose and asserted against the descriptors: the registry is the
/// obligation and the descriptors are the discharge, so the test is only meaningful if the
/// two lists are written independently. The package also carries two `diagnostic` rows —
/// `LOAD-REGIONAL-ADMISSION` and `PERF-BASELINE-CODECS` — which are recorded rather than
/// blocking and have no descriptor yet; they are deliberately absent from this list, so
/// adding one is a visible edit rather than a silent widening.
const OWED_BLOCKING_GATES: [&str; 2] = ["LOAD-STREAM-SOCKETS", "PERF-BASELINE-JOURNAL-FOLD"];

#[test]
fn every_blocking_gate_this_package_owes_has_a_descriptor() {
    let declared: BTreeSet<String> = descriptors()
        .into_iter()
        .flat_map(|(_, descriptor)| descriptor.gates.into_iter().map(|gate| gate.id))
        .collect();
    for gate in OWED_BLOCKING_GATES {
        assert!(
            declared.contains(gate),
            "gate `{gate}` is blocking and assigned to `{TARGET}`, and no descriptor \
             implements it; it would be reported as a pending row, never as a pass"
        );
    }
}

/// `id` is the file name. A descriptor whose id disagreed with its path would be found by
/// the registry under one name and by the executor under another.
#[test]
fn every_descriptor_is_named_by_its_file_and_verifies_against_a_real_tier() {
    let tiers = known_tiers();
    for (name, descriptor) in descriptors() {
        assert_eq!(descriptor.id, name);
        assert_eq!(descriptor.target, TARGET);
        descriptor
            .verify(&tiers, Phase::SourceRewrite)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
    }
}

/// A threshold with no source is a guess, and a guess that blocks a release is worse than
/// no gate at all.
#[test]
fn every_blocking_gate_names_the_record_its_threshold_comes_from() {
    for (name, descriptor) in descriptors() {
        for gate in &descriptor.gates {
            if gate.blocking {
                assert_eq!(gate.kind, GateKind::Budget, "{name}/{}", gate.id);
                assert!(
                    gate.source.len() >= 8,
                    "{name}/{}: a blocking budget must name its source",
                    gate.id
                );
            }
        }
    }
}

/// The socket campaign is about one number. A campaign that held nine hundred sockets and
/// passed would tell nobody that the thousandth is where the per-connection budget breaks.
#[test]
fn the_socket_campaign_holds_the_thousand_sockets_the_gate_is_about() {
    let (_, sockets) = descriptors()
        .into_iter()
        .find(|(name, _)| name == "stream-sockets")
        .expect("the socket campaign exists");
    assert_eq!(
        sockets.concurrency,
        Some(1_000),
        "PERF-11 is a claim about 1000 sustained sockets on a 1 vCPU / 2 GiB task"
    );
    assert_eq!(sockets.arrival, aex_load_harness::Arrival::Closed);
    assert!(sockets.mix.is_normalized());
}

/// The fold baseline is a single-shape measurement, and the shape is the whole point: ten
/// thousand events. Folding a shorter journal inside the same budget would hide exactly the
/// quadratic regression the gate exists to catch.
#[test]
fn the_fold_baseline_measures_the_ten_thousand_event_shape() {
    let (_, fold) = descriptors()
        .into_iter()
        .find(|(name, _)| name == "journal-fold")
        .expect("the fold baseline exists");
    assert!(
        fold.mix.0.keys().any(|shape| shape.contains("10000")),
        "the fold mix must name the event count its budget is measured at: {:?}",
        fold.mix.0.keys().collect::<Vec<_>>()
    );
    assert!(
        fold.gates
            .iter()
            .any(|gate| gate.id == "PERF-BASELINE-JOURNAL-FOLD" && gate.blocking),
        "the only PERF-BASELINE row that is a budget must stay blocking"
    );
}

/// A campaign whose resources may outlive the campaign leaves residue nobody attributes.
#[test]
fn every_campaign_declares_a_ttl_longer_than_itself() {
    for (name, descriptor) in descriptors() {
        assert!(
            descriptor.ttl_ms > descriptor.duration_ms,
            "{name}: a TTL at or below the duration expires the campaign's own resources"
        );
    }
}

// ---------------------------------------------------------------------------
// execution
// ---------------------------------------------------------------------------

/// The plane descriptor every executing campaign binds to.
const PLANE_VAR: &str = "AEX_LIVE_PLANE";

/// Runs the campaigns against a deployed plane.
///
/// Reads its binding through `required_env!`, which panics with the variable's name when it
/// is absent. That is the correct verdict: this target is selected only by the load and soak
/// profiles, and a load profile that cannot reach a plane has not proved anything.
#[test]
fn campaigns_execute_against_the_bound_plane() {
    let plane = aex_test_harness::required_env!(PLANE_VAR);
    let loaded = descriptors();
    let budget: u64 = loaded
        .iter()
        .filter_map(|(_, descriptor)| descriptor.budget_micro_usd)
        .sum();
    let run = aex_test_harness::TestRun::mint(
        aex_test_harness::Lane::Load,
        "regional-services",
        budget,
    );
    panic!(
        "`{TARGET}` holds {} campaigns and run identity {} for plane `{plane}`, and cannot \
         execute one: the socket campaign needs a listening stream endpoint and the fold \
         baseline needs a session whose journal can be grown to ten thousand events, and \
         neither exists until the plane serves the release under test. Reported as a \
         failure rather than as a green run of zero work.",
        loaded.len(),
        run.id().as_str()
    );
}
