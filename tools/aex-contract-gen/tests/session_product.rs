//! Exact clean-cut launch surface and security/product invariants.

use std::collections::BTreeSet;

use aex_contract_gen::generate_to_memory;
use aex_contract_gen::load::repo_root;

fn json(path: &str) -> serde_json::Value {
    let tree = generate_to_memory(&repo_root()).expect("contract generation");
    serde_json::from_slice(
        tree.bytes(path)
            .unwrap_or_else(|| panic!("missing `{path}`")),
    )
    .unwrap_or_else(|error| panic!("`{path}` is not JSON: {error}"))
}

fn operations(plane: &str) -> BTreeSet<String> {
    json("api/generated/bundle.json")["planes"][plane]["operations"]
        .as_array()
        .expect("operations")
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
fn launch_routes_are_the_exact_fourteen_plus_nineteen_allowlist() {
    let central = BTreeSet::from(
        [
            "api_key_create",
            "api_key_revoke",
            "api_keys_list",
            "auth_config_get",
            "billing_balance_get",
            "billing_payment_method_delete",
            "billing_payment_method_session_create",
            "billing_payment_methods_list",
            "billing_top_up_checkout_create",
            "billing_transactions_list",
            "billing_usage_get",
            "dashboard_bootstrap_get",
            "dashboard_session_create",
            "dashboard_session_delete",
        ]
        .map(str::to_owned),
    );
    let regional = BTreeSet::from(
        [
            "registry_files_delete",
            "registry_files_download_create",
            "registry_files_get",
            "registry_files_list",
            "registry_files_put",
            "session_cancel",
            "session_create",
            "session_delete",
            "session_get",
            "session_message_send",
            "session_messages_list",
            "session_messages_stream",
            "session_telemetry_download_create",
            "session_telemetry_replay",
            "session_telemetry_stream",
            "session_terminate",
            "sessions_list",
            "upload_complete",
            "upload_create",
        ]
        .map(str::to_owned),
    );
    assert_eq!(operations("central"), central);
    assert_eq!(operations("regional"), regional);
}

#[test]
fn message_admission_is_text_only_but_supports_native_structured_output() {
    let request = json("api/generated/schemas/MessageSendRequest.json");
    let keys: BTreeSet<_> = request["properties"]
        .as_object()
        .expect("properties")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from(["deadline", "maxSpendCents", "responseFormat", "text"])
    );
    for forbidden in ["attachment", "attachments", "image", "file", "files"] {
        assert!(!keys.contains(forbidden));
    }
    assert_eq!(request["required"], serde_json::json!(["text"]));
    assert_eq!(request["properties"]["text"]["maxLength"], 24_576);

    let structured = json("api/generated/schemas/ResponseFormatKind.json");
    assert_eq!(
        structured["enum"],
        serde_json::json!(["text", "json_schema"])
    );
}

#[test]
fn providers_are_the_seven_official_candidates_and_no_credential_crud_survives() {
    assert_eq!(
        json("api/generated/schemas/ProviderId.json")["enum"],
        serde_json::json!([
            "openai",
            "anthropic",
            "deepseek",
            "xai",
            "meta",
            "moonshotai",
            "alibaba"
        ])
    );
    let regional = operations("regional");
    assert!(!regional.iter().any(|id| id.contains("provider_credential")));
    let request = json("api/generated/schemas/SessionCreateRequest.json");
    assert!(request["properties"].get("providerApiKey").is_some());
    let session = json("api/generated/schemas/Session.json");
    assert!(session.to_string().find("providerApiKey").is_none());
}

#[test]
fn files_are_current_value_only_and_session_mounts_are_frozen_by_name_and_path() {
    let regional = operations("regional");
    for id in &regional {
        assert!(!id.contains("version"));
        assert!(!id.contains("restore"));
        assert!(!id.contains("files_live"));
    }
    let mount = json("api/generated/schemas/WorkspaceFileMount.json");
    assert_eq!(
        mount["properties"]
            .as_object()
            .expect("properties")
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["name", "path"])
    );
    assert!(
        json("api/generated/schemas/RegisteredFile.json")
            .to_string()
            .find("version")
            .is_none()
    );
}

#[test]
fn sandbox_mcp_streaming_telemetry_and_subagent_limits_are_explicit() {
    let create = json("api/generated/schemas/SessionCreateRequest.json");
    for field in ["sandbox", "mcpServers", "registered"] {
        assert!(
            create["properties"].get(field).is_some(),
            "missing `{field}`"
        );
    }
    let lifecycle = json("api/generated/schemas/SessionLifecyclePolicy.json");
    assert_eq!(lifecycle["properties"]["maximumSubagents"]["minimum"], 12);
    assert_eq!(lifecycle["properties"]["maximumSubagents"]["maximum"], 12);
    assert_eq!(
        lifecycle["properties"]["maximumSubagentDepth"]["minimum"],
        3
    );
    let sandbox_status = json("api/generated/schemas/SandboxStatus.json");
    let lifecycle_states = sandbox_status["enum"]
        .as_array()
        .expect("enum")
        .iter()
        .map(|state| state.as_str().expect("sandbox state"))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        lifecycle_states,
        BTreeSet::from([
            "disabled",
            "requested",
            "ready",
            "suspending",
            "suspended",
            "resuming",
            "lost",
        ])
    );
    let regional = operations("regional");
    for route in [
        "session_messages_stream",
        "session_telemetry_stream",
        "session_telemetry_replay",
        "session_telemetry_download_create",
    ] {
        assert!(regional.contains(route));
    }
}

#[test]
fn essential_billing_has_no_raw_card_or_retired_product_surface() {
    let billing = [
        "BillingBalance",
        "PaymentMethod",
        "PaymentMethodSessionRequest",
        "TopUpCheckoutRequest",
        "BillingTransaction",
        "BillingUsageItem",
        "BillingUsageCoverage",
    ];
    for schema in billing {
        let body = json(&format!("api/generated/schemas/{schema}.json"));
        let keys: BTreeSet<_> = body["properties"]
            .as_object()
            .expect("properties")
            .keys()
            .map(|key| key.to_ascii_lowercase())
            .collect();
        for forbidden in [
            "pan",
            "cvc",
            "cardnumber",
            "clientsecret",
            "setupintentsecret",
        ] {
            assert!(!keys.contains(forbidden), "{schema} leaked `{forbidden}`");
        }
    }
    let central = operations("central");
    for retired in [
        "portal",
        "auto_topup",
        "statement",
        "subscription",
        "invoice",
        "organization",
        "membership",
        "device",
    ] {
        assert!(
            !central.iter().any(|id| id.contains(retired)),
            "retired `{retired}` route survived"
        );
    }
}
