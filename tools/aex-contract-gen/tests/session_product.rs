//! The prelaunch session-centric public product boundary.
//!
//! These assertions are deliberately independent of the checked-in generated
//! files. They regenerate from the authored YAML so a removed route, leaked
//! execution identity, or broadened resource category fails before release.

use std::collections::BTreeSet;

use aex_contract_gen::generate_to_memory;
use aex_contract_gen::load::repo_root;

fn generated_json(path: &str) -> serde_json::Value {
    let tree = generate_to_memory(&repo_root()).expect("contract generation");
    serde_json::from_slice(
        tree.bytes(path)
            .unwrap_or_else(|| panic!("missing `{path}`")),
    )
    .unwrap_or_else(|error| panic!("`{path}` is not JSON: {error}"))
}

fn regional_operations() -> BTreeSet<String> {
    generated_json("api/generated/bundle.json")["planes"]["regional"]["operations"]
        .as_array()
        .expect("regional operations")
        .iter()
        .map(|operation| {
            operation["operationId"]
                .as_str()
                .expect("operationId")
                .to_owned()
        })
        .collect()
}

#[test]
fn the_regional_surface_is_session_centric_and_has_one_file_registry() {
    let operations = regional_operations();
    assert_eq!(operations.len(), 98, "regional route ledger drifted");

    for required in [
        "session_cancel",
        "session_suspend",
        "session_resume",
        "session_terminate",
        "session_delete",
        "session_files_live_list",
        "session_files_live_stat",
        "session_files_live_download_create",
        "session_files_live_download_part_get",
        "session_files_live_download_complete",
        "session_files_live_download_delete",
        "session_files_live_upload_create",
        "session_files_live_upload_get",
        "session_files_live_upload_part_put",
        "session_files_live_upload_complete",
        "session_files_live_upload_delete",
        "registry_files_list",
        "registry_files_get",
        "registry_files_put",
        "registry_files_delete",
        "registry_files_download_create",
        "upload_create",
        "upload_parts_grant",
        "upload_complete",
        "upload_abort",
    ] {
        assert!(operations.contains(required), "missing `{required}`");
    }

    for retired in [
        "session_run_get",
        "session_runs_list",
        "session_stop",
        "session_persist",
        "session_workspace_discard",
        "session_credential_rebind",
        "session_trash",
        "session_restore",
        "session_purge",
        "session_files_persisted_list",
        "session_files_persisted_stat",
        "session_files_persisted_download_create",
        "secrets_list",
        "secret_get",
        "secret_put",
        "secret_delete",
        "secret_revoke",
        "registry_skills_list",
        "registry_skills_get",
        "registry_skills_put",
        "registry_skills_delete",
        "registry_tools_list",
        "registry_tools_get",
        "registry_tools_put",
        "registry_tools_delete",
        "registry_instructions_list",
        "registry_instructions_get",
        "registry_instructions_put",
        "registry_instructions_delete",
        "registry_mcp_servers_list",
        "registry_mcp_servers_get",
        "registry_mcp_servers_put",
        "registry_mcp_servers_delete",
    ] {
        assert!(
            !operations.contains(retired),
            "retired `{retired}` survived"
        );
    }
}

#[test]
fn message_admission_is_text_only_and_returns_the_session() {
    let request = generated_json("api/generated/schemas/MessageSendRequest.json");
    let properties = request["properties"]
        .as_object()
        .expect("request properties");
    assert_eq!(
        properties
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["deadline", "maxSpendCents", "text"])
    );
    assert_eq!(request["required"], serde_json::json!(["text"]));
    assert!(
        request["description"]
            .as_str()
            .expect("request description")
            .contains("1000-cent default"),
        "the exact omitted spend default must remain public contract"
    );
    assert!(
        properties["deadline"]["description"]
            .as_str()
            .expect("deadline description")
            .contains("cannot exceed the session's remaining `expiresAt`/drain fence"),
        "omitted and explicit deadline semantics must remain public contract"
    );

    let response = generated_json("api/generated/schemas/MessageSendResult.json");
    let properties = response["properties"]
        .as_object()
        .expect("result properties");
    assert_eq!(
        properties
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["message", "session"])
    );

    let tree = generate_to_memory(&repo_root()).expect("contract generation");
    assert!(tree.bytes("api/generated/schemas/Run.json").is_none());
}

#[test]
fn no_public_schema_or_identifier_exposes_a_run_or_turn_identity() {
    let tree = generate_to_memory(&repo_root()).expect("contract generation");
    for path in [
        "api/generated/bundle.json",
        "api/generated/registries/ids.json",
        "api/generated/schemas/Message.json",
        "api/generated/schemas/Observation.json",
        "api/generated/schemas/UsageAttribution.json",
        "api/generated/schemas/ApprovalBoundCall.json",
    ] {
        let text = std::str::from_utf8(
            tree.bytes(path)
                .unwrap_or_else(|| panic!("missing `{path}`")),
        )
        .expect("generated JSON is UTF-8");
        for leaked in ["runId", "turnId", "aex:schema:Run", "\"kind\": \"run\""] {
            assert!(!text.contains(leaked), "`{path}` leaked `{leaked}`");
        }
    }
}

#[test]
fn credentials_and_identity_have_one_explicit_authority_each() {
    let scopes = generated_json("api/generated/registries/scopes.json");
    let scopes: BTreeSet<_> = scopes["scopes"]
        .as_array()
        .expect("scope rows")
        .iter()
        .map(|row| row["scope"].as_str().expect("scope"))
        .collect();
    assert!(scopes.contains("provider_credentials:read"));
    assert!(scopes.contains("provider_credentials:write"));
    assert!(!scopes.iter().any(|scope| scope.starts_with("secrets:")));

    let providers = generated_json("api/generated/schemas/IdentityProvider.json");
    assert_eq!(providers["enum"], serde_json::json!(["google"]));
}

#[test]
fn session_lifecycle_is_automatic_but_observable_without_a_turn_resource() {
    let session = generated_json("api/generated/schemas/Session.json");
    let properties = session["properties"]
        .as_object()
        .expect("session properties");
    for field in [
        "activeMessageId",
        "activeMaxSpendCents",
        "activeDeadline",
        "launchedAt",
        "expiresAt",
        "idleSince",
        "suspendAt",
        "suspendedAt",
        "terminatedAt",
        "terminationReason",
    ] {
        assert!(properties.contains_key(field), "Session misses `{field}`");
    }
    for retired in ["continuity", "lineage", "currentTurn", "run"] {
        assert!(
            !properties.contains_key(retired),
            "Session retains `{retired}`"
        );
    }

    let resolved = generated_json("api/generated/schemas/ResolvedConfig.json");
    assert!(resolved["properties"].get("lifecycle").is_some());
    let lifecycle = generated_json("api/generated/schemas/SessionLifecyclePolicy.json");
    assert_eq!(
        lifecycle["properties"]["idleSuspendAfterSeconds"]["minimum"],
        180
    );
    assert_eq!(
        lifecycle["properties"]["idleSuspendAfterSeconds"]["maximum"],
        180
    );
    assert_eq!(
        lifecycle["properties"]["maximumLifetimeSeconds"]["minimum"],
        28_800
    );
    assert_eq!(
        lifecycle["properties"]["maximumLifetimeSeconds"]["maximum"],
        28_800
    );
}
