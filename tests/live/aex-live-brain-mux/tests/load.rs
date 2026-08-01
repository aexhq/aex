//! The brain-core load and soak executors.
//!
//! This target is selected by the `load` and `soak` nextest profiles and by no other. The
//! unit lane excludes it by package (`default-filter = 'not package(/^aex-live-/)'`), which
//! is a declared configuration fact rather than a self-skip: nothing here checks an
//! environment variable and returns green.
//!
//! Two halves, deliberately separated.
//!
//! 1. **Descriptor conformance** runs with no plane at all. It proves that every gate the
//!    registry assigns brain-core has a descriptor, that each descriptor parses under the
//!    same rules the schema declares, and that the child counts the plan names are actually
//!    covered. A campaign whose descriptor is wrong measures the wrong thing, and finding
//!    that out after a 12-hour soak is finding it out too late.
//! 2. **Execution** needs a deployed plane. It reads its binding through
//!    `aex_test_harness::required_env!`, which panics with the variable's name when it is
//!    absent — a live prerequisite is a failure, never a skip.

use aex_load_harness::{GateKind, WorkloadDescriptor, WorkloadError};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The descriptor directory this stream owns.
fn workload_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../load/workloads/brain-core")
        .canonicalize()
        .expect("the brain-core workload directory exists")
}

fn descriptors() -> Vec<(String, WorkloadDescriptor)> {
    let mut loaded = Vec::new();
    let entries = std::fs::read_dir(workload_dir()).expect("the workload directory is readable");
    for entry in entries {
        let path = entry.expect("a readable directory entry").path();
        if path.extension().is_none_or(|it| it != "toml") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("a readable descriptor");
        let name = path
            .file_stem()
            .and_then(|it| it.to_str())
            .expect("a UTF-8 file name")
            .to_owned();
        let descriptor = WorkloadDescriptor::parse(&name, &text)
            .unwrap_or_else(|error: WorkloadError| panic!("{name}: {error}"));
        loaded.push((name, descriptor));
    }
    loaded.sort_by(|left, right| left.0.cmp(&right.0));
    assert!(!loaded.is_empty(), "brain-core owns load workloads");
    loaded
}

/// Every gate `release/policy/workload-registry.toml` assigns brain-core.
///
/// Hand-written here on purpose and asserted against the descriptors: the registry is the
/// obligation and the descriptors are the discharge, so the test is only meaningful if the
/// two lists are written independently.
const OWED_GATES: [&str; 6] = [
    "LOAD-100-COMPLETE",
    "LOAD-100-BUDGETS",
    "LOAD-200-SAFE",
    "LOAD-500-BOUNDED",
    "LEAK-SOAK-12H",
    "LEAK-SOAK-24H",
];

#[test]
fn every_owed_gate_has_a_descriptor() {
    let declared: BTreeSet<String> = descriptors()
        .into_iter()
        .flat_map(|(_, descriptor)| descriptor.gates.into_iter().map(|gate| gate.id))
        .collect();
    for gate in OWED_GATES {
        assert!(
            declared.contains(gate),
            "gate `{gate}` is owed by brain-core but no descriptor implements it; it would be \
             reported as a pending row, never as a pass"
        );
    }
}

/// `id` is the file name. A descriptor whose id disagreed with its path would be found by
/// the registry under one name and by the executor under another.
#[test]
fn every_descriptor_is_named_by_its_file() {
    for (name, descriptor) in descriptors() {
        assert_eq!(descriptor.id, name);
        assert_eq!(descriptor.owner, "brain-core");
        assert_eq!(descriptor.target, "aex-live-brain-mux");
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

/// The plan names seven child counts. Covering six of them would leave a page boundary
/// untested and nobody would notice, so the set is asserted rather than sampled.
#[test]
fn the_declared_child_counts_are_all_covered() {
    let (_, fanout) = descriptors()
        .into_iter()
        .find(|(name, _)| name == "brain-swarm-fanout")
        .expect("the fanout campaign exists");
    for count in [1_u32, 5, 10, 15, 50, 100, 200] {
        let shape = format!("fanout_{count}");
        assert!(
            fanout.mix.0.contains_key(&shape),
            "child count {count} has no shape in the fanout mix"
        );
    }
    assert!(
        fanout.mix.is_normalized(),
        "the fanout mix weights must sum to 1.0"
    );
}

/// The overload campaign must offer more than the safety cap admits, or it is not an
/// overload campaign.
#[test]
fn the_overload_campaign_offers_past_the_safety_cap() {
    let (_, overload) = descriptors()
        .into_iter()
        .find(|(name, _)| name == "brain-500-offered")
        .expect("the overload campaign exists");
    let rate = overload.rate_per_s.expect("open arrival declares a rate");
    // Counted up in whole seconds rather than cast down from a float, so the assertion is
    // about the campaign rather than about what this platform does with an out-of-range
    // cast. The loop runs once per declared second.
    let seconds = overload.duration_ms / 1_000;
    let mut offered = 0.0_f64;
    for _ in 0..seconds {
        offered += rate;
    }
    assert!(
        offered >= 500.0,
        "the campaign offers {offered:.0} units; the declared ceiling is 500"
    );
}

/// Both soak windows exist and the longer one is the diagnostic. A blocking 24-hour gate
/// would make every release wait a day for evidence a 12-hour window already gives.
#[test]
fn the_soak_windows_are_twelve_blocking_and_twenty_four_recorded() {
    let loaded = descriptors();
    let twelve = loaded
        .iter()
        .find(|(name, _)| name == "brain-soak-12h")
        .expect("the 12-hour soak exists");
    let twenty_four = loaded
        .iter()
        .find(|(name, _)| name == "brain-soak-24h")
        .expect("the 24-hour soak exists");
    assert_eq!(twelve.1.duration_ms, 12 * 60 * 60 * 1_000);
    assert_eq!(twenty_four.1.duration_ms, 24 * 60 * 60 * 1_000);
    assert!(twelve.1.gates.iter().any(|gate| gate.blocking));
    assert!(
        twenty_four.1.gates.iter().all(|gate| !gate.blocking),
        "the 24-hour window is recorded, never blocking"
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
    let run = aex_test_harness::TestRun::mint(aex_test_harness::Lane::Load, "brain-core", budget);
    panic!(
        "brain-core holds {} campaigns and run identity {} for plane `{plane}`, and cannot \
         execute one: the mux composes its runtimes, admission, cache, drain and \
         measurement, but its wake loop still needs the queue and table bindings the \
         delivery stream owns. Reported as a failure rather than as a green run of zero \
         work.",
        loaded.len(),
        run.id().as_str()
    );
}
