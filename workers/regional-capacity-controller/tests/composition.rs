//! Fail-closed composition contract for the capacity controller.

use std::collections::BTreeMap;

use regional_capacity_controller::{Config, RegionalCapacityControllerConfigError, keys};

fn complete() -> BTreeMap<&'static str, String> {
    BTreeMap::from([
        (keys::PLANE, "dev".to_owned()),
        (keys::REGION, "eu-west-1".to_owned()),
        (keys::RELEASE_DIGEST, format!("sha256:{}", "a".repeat(64))),
        (keys::AUTHORITY_TABLE, "dev-capacity-authority".to_owned()),
        (keys::PROJECTION_TABLE, "dev-authz-projection".to_owned()),
    ])
}

#[test]
fn every_binding_is_required_and_tables_must_be_distinct() {
    for key in keys::ALL {
        let mut vars = complete();
        vars.remove(key);
        assert_eq!(
            Config::from_lookup(|name| vars.get(name).cloned()),
            Err(RegionalCapacityControllerConfigError::Missing(key))
        );
    }
    let mut vars = complete();
    vars.insert(keys::PROJECTION_TABLE, "dev-capacity-authority".to_owned());
    assert!(Config::from_lookup(|name| vars.get(name).cloned()).is_err());
}

#[test]
fn plane_region_and_release_identity_are_closed() {
    for (key, value) in [
        (keys::PLANE, "stage"),
        (keys::REGION, "mars-1"),
        (keys::RELEASE_DIGEST, "latest"),
    ] {
        let mut vars = complete();
        vars.insert(key, value.to_owned());
        assert!(Config::from_lookup(|name| vars.get(name).cloned()).is_err());
    }
}

/// The bootstrap command central control sends is the one this controller
/// decodes, and its answer is the one central decodes back.
///
/// Central may not link `aex-capacity-dynamodb`: the capacity producer is a
/// separate Cargo feature behind a disjoint IAM keyspace, so that central
/// cannot write a limit row even by mistake. The cost of that is a second
/// spelling of one command, in `aex-internal-contracts`. This is what stops the
/// two spellings from drifting — a rename on either side fails here rather than
/// turning every provision into a refusal nobody is watching for.
#[test]
fn the_command_central_sends_is_the_command_this_controller_decodes() {
    use aex_capacity_dynamodb::CapacityCommand;
    use aex_internal_contracts::capacity::{ALREADY_EXISTS, CapacityBootstrap, CapacityOutcome};
    use aex_wire::ids::{PrefixedId as _, Uuid7, WorkspaceId};

    let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [7; 10]));

    let central = serde_json::to_value(CapacityBootstrap::new(workspace)).expect("central encodes");
    let controller = serde_json::to_value(CapacityCommand::Bootstrap {
        workspace_id: workspace,
    })
    .expect("the controller encodes");
    assert_eq!(
        central, controller,
        "the two spellings of `bootstrap` drifted"
    );

    // And the controller can read what central wrote, not merely produce the
    // same bytes for its own value.
    let decoded: CapacityCommand = serde_json::from_value(central).expect("the controller decodes");
    assert_eq!(
        decoded,
        CapacityCommand::Bootstrap {
            workspace_id: workspace
        }
    );

    // The answer, in the other direction. `already_exists` is the refusal a
    // re-bootstrap of a reconciled workspace produces, and central must read it
    // as a satisfied precondition rather than a failure.
    for (response, materialised) in [
        (
            regional_capacity_controller::ControllerResponse::Refused {
                code: ALREADY_EXISTS,
                message: "workspace capacity already exists".to_owned(),
            },
            true,
        ),
        (
            regional_capacity_controller::ControllerResponse::Refused {
                code: "revision_conflict",
                message: "stale".to_owned(),
            },
            false,
        ),
    ] {
        let encoded = serde_json::to_value(&response).expect("the controller encodes its answer");
        let read: CapacityOutcome = serde_json::from_value(encoded).expect("central decodes it");
        assert_eq!(read.set_is_materialised(), materialised, "{response:?}");
    }
}
