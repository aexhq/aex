//! Composition and key-shape evidence for the export launcher.
//!
//! The plan's load-bearing claim about this deployable is negative: it holds
//! **no observation read permission at all**, so a launcher bug cannot become a
//! data path. That claim is only worth anything if linking a read, write or
//! delete capability into this process refuses to start, which is what the
//! capability cases below prove against the shared role table rather than a
//! local copy.
//!
//! The reconciliation scenarios (a lost claim, an ambiguous `RunTask`) live in
//! `src/launcher.rs`, next to the logic they constrain, over a hand-written fake
//! of both ports.

use aex_observation_domain::keys::{ControlDomain, control_pk, control_sk};
use aex_observation_store_dynamodb::composition::{Capability, Role, assert_grant};
use aex_observation_store_dynamodb::expressions::Index;
use aex_observation_store_dynamodb::health::Probe;
use aex_wire::ids::{ExportId, PrefixedId, Uuid7, WorkspaceId};
use aex_wire::types::Timestamp;

/// The launcher's whole grant, restated so a widening is a test failure.
const GRANT: &[Capability] = &[Capability::LaunchExportTasks];

#[test]
fn the_launcher_grant_is_exactly_launch_export_tasks() {
    assert_eq!(Role::ExportLauncher.granted(), GRANT);
    assert_grant(Role::ExportLauncher, GRANT).expect("its own grant is admitted");
}

#[test]
fn cannot_link_any_capability_outside_the_exact_grant() {
    for capability in Role::ExportLauncher.denied() {
        assert!(
            !Role::ExportLauncher.holds(capability),
            "`{}` must not be in the launcher grant",
            capability.as_str()
        );
        let violation = assert_grant(Role::ExportLauncher, &[capability])
            .expect_err("a capability outside the grant refuses to start");
        assert_eq!(violation.capability, capability);
        assert_eq!(violation.role, Role::ExportLauncher);
    }
}

#[test]
fn every_capability_is_classified_as_granted_or_denied() {
    let denied = Role::ExportLauncher.denied();
    for capability in Capability::ALL {
        assert_ne!(
            Role::ExportLauncher.holds(*capability),
            denied.contains(capability),
            "{capability:?} must appear on exactly one side of the grant"
        );
    }
    assert_eq!(denied.len() + GRANT.len(), Capability::ALL.len());
}

#[test]
fn the_only_dependency_this_deployable_proves_is_the_export_cluster() {
    assert_eq!(Probe::ExportCluster.as_str(), "export_cluster");
    // A probe naming the observation bucket or the cursor key ring would mean
    // the launcher had been given a data dependency it holds no capability for.
    for absent in [
        Probe::ObservationBucket,
        Probe::ObservationTable,
        Probe::CursorKeyRing,
    ] {
        assert_ne!(Probe::ExportCluster, absent);
    }
}

#[test]
fn the_due_scan_reads_the_sparse_control_index_and_never_the_dense_ones() {
    assert_eq!(Index::Control.as_str(), "gsi_control");
    assert_eq!(Index::Control.partition_key(), "cPk");
    assert_eq!(Index::Control.sort_key(), "cSk");
    assert!(
        !Index::Control.is_dense(),
        "a dense index would put every observation in the launcher's path"
    );
    assert!(
        Index::Control.projection().is_empty(),
        "the control index is KEYS_ONLY; a projection would hand the launcher observation data"
    );
}

#[test]
fn the_launcher_scans_exactly_the_export_launch_control_domain() {
    assert_eq!(ControlDomain::ExportLaunch.as_str(), "export.launch");
    assert_eq!(
        control_pk(ControlDomain::ExportLaunch, 7),
        "CTRL#export.launch#07"
    );
    assert_eq!(
        ControlDomain::parse("export.launch"),
        Some(ControlDomain::ExportLaunch)
    );
    // Reaping is a different deployment's domain; sharing the partition would
    // let one bug launch what the other meant to delete.
    assert_ne!(ControlDomain::ExportLaunch, ControlDomain::ExportReap);
}

#[test]
fn a_due_sort_key_orders_lexicographically_by_instant() {
    let earlier = Timestamp::from_unix_millis(1_700_000_000_000).expect("representable");
    let later = Timestamp::from_unix_millis(1_700_000_000_001).expect("representable");
    let export = ExportId::from_uuid7(Uuid7::compose(9, [9; 10]));
    let first = control_sk(earlier, &export.to_string());
    let second = control_sk(later, &export.to_string());
    assert!(first < second, "{first} !< {second}");
    // The empty item id renders the exclusive floor of an instant's whole group,
    // which is what the due window's upper bound is built from.
    assert!(control_sk(later, "") < second);
    assert!(first < control_sk(later, ""));
}

#[test]
fn the_state_row_key_is_derived_from_the_shared_key_template() {
    let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]));
    let export = ExportId::from_uuid7(Uuid7::compose(2, [2; 10]));
    let pk = aex_observation_domain::keys::export_pk(workspace, export);
    assert_eq!(pk, format!("EXPORT#{workspace}#{export}"));
    assert!(pk.starts_with("EXPORT#"));
}
