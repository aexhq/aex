//! The determinism and drift gates.
//!
//! "Regenerate twice" is only meaningful if nothing in the pipeline reads
//! ambient state, so these tests generate wholly in memory and compare bytes.
//! `checked_in_output_matches_regeneration` is what makes a hand edit of a
//! generated file a red test rather than a silent divergence.

use std::collections::{BTreeMap, BTreeSet};

use aex_contract_gen::classify::{Classification, classify};
use aex_contract_gen::load::repo_root;
use aex_contract_gen::{emit, generate_to_memory, jcs, load};

fn copy_authored_contract(root: &std::path::Path) {
    let repository = repo_root();
    for entry in walkdir::WalkDir::new(repository.join("api"))
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        let relative = entry
            .path()
            .strip_prefix(&repository)
            .expect("inside repository");
        if relative.starts_with("api/generated") {
            continue;
        }
        let target = root.join(relative);
        std::fs::create_dir_all(target.parent().expect("file parent")).expect("create parent");
        std::fs::copy(entry.path(), target).expect("copy authored contract input");
    }
    std::fs::copy(
        repository.join("rust-toolchain.toml"),
        root.join("rust-toolchain.toml"),
    )
    .expect("copy toolchain pin");
}

fn replace_exactly_once(authored: &str, from: &str, to: &str, mutation: &str) -> String {
    assert_eq!(
        authored.matches(from).count(),
        1,
        "the {mutation} fixture requires exactly one `{from}` anchor"
    );
    let changed = authored.replacen(from, to, 1);
    assert_ne!(changed, authored, "the {mutation} fixture rewrote nothing");
    changed
}

#[test]
fn regenerate_produces_byte_identical_output() {
    let root = repo_root();
    let first = generate_to_memory(&root).expect("first generation");
    let second = generate_to_memory(&root).expect("second generation");
    assert_eq!(first.file_names(), second.file_names());
    assert!(!first.is_empty(), "the generator produced nothing");
    for name in first.file_names() {
        assert_eq!(
            first.bytes(name),
            second.bytes(name),
            "`{name}` differs between two runs"
        );
    }
}

#[test]
fn checked_in_output_matches_regeneration() {
    let root = repo_root();
    let drift = generate_to_memory(&root)
        .expect("generation")
        .diff_against_disk(&root);
    assert!(
        drift.is_empty(),
        "stale generated output:\n{}\nrun `cargo run -p aex-contract-gen -- build`",
        drift.join("\n")
    );
}

#[test]
fn no_orphaned_generated_file_survives_on_disk() {
    // A schema that stops being authored leaves its published document behind.
    // Comparing only the files the generator still produces cannot see that, so
    // a deleted schema would keep serving from `api/generated/schemas/` forever.
    let root = repo_root();
    let stale = generate_to_memory(&root)
        .expect("generation")
        .stale_files(&root);
    assert!(
        stale.is_empty(),
        "orphaned generated output:\n{}\nrun `cargo run -p aex-contract-gen -- build`",
        stale.join("\n")
    );
}

#[test]
fn the_digest_is_stable_across_repeated_loads() {
    let root = repo_root();
    let first = emit::contract_digest(&load::load(&root).expect("load"));
    let second = emit::contract_digest(&load::load(&root).expect("load"));
    assert_eq!(first, second);
    assert!(first.starts_with("sha256:"), "{first}");
    assert_eq!(first.len(), 71, "{first}");
}

#[test]
fn the_lock_file_records_the_digest_the_bundle_actually_has() {
    let root = repo_root();
    let tree = generate_to_memory(&root).expect("generation");
    let bundle: serde_json::Value =
        serde_json::from_slice(tree.bytes("api/generated/bundle.json").expect("bundle"))
            .expect("bundle json");
    let lock: serde_json::Value =
        serde_json::from_slice(tree.bytes("api/generated/bundle.lock.json").expect("lock"))
            .expect("lock json");
    assert_eq!(lock["contractDigest"], jcs::digest(&bundle));
    let sources = lock["sources"].as_object().expect("sources");
    assert!(sources.len() > 20, "only {} inputs recorded", sources.len());
    for path in sources.keys() {
        assert!(!path.contains('\\'), "`{path}` is not a `/` path");
        assert!(!path.contains(':'), "`{path}` leaks an absolute path");
    }
}

#[test]
fn operation_ids_are_unique_and_snake_case() {
    let ir = load::load(&repo_root()).expect("load");
    let operations = ir.operations();
    let ids: BTreeSet<&str> = ir
        .operations()
        .iter()
        .map(|operation| operation.id.as_str())
        .collect();
    assert_eq!(
        ids.len(),
        operations.len(),
        "operationIds are not globally unique"
    );
    for id in &ids {
        assert!(
            id.chars().all(|character| character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '_'),
            "`{id}` is not snake_case"
        );
    }
}

#[test]
fn every_route_registry_row_has_a_scenario_owner_without_polluting_the_wire_bundle() {
    let root = repo_root();
    let tree = generate_to_memory(&root).expect("generation");
    let routes: serde_json::Value = serde_json::from_slice(
        tree.bytes("api/generated/registries/routes.json")
            .expect("route registry"),
    )
    .expect("route registry json");
    for route in routes["routes"].as_array().expect("route rows") {
        assert!(
            route["scenarios"]
                .as_array()
                .is_some_and(|owners| !owners.is_empty()),
            "{} has no scenario owner",
            route["operationId"]
        );
    }

    let bundle: serde_json::Value =
        serde_json::from_slice(tree.bytes("api/generated/bundle.json").expect("bundle"))
            .expect("bundle json");
    for plane in ["central", "regional"] {
        for route in bundle["planes"][plane]["operations"]
            .as_array()
            .expect("bundle route rows")
        {
            assert!(
                route.get("scenarios").is_none(),
                "scenario selection leaked into the {plane} wire contract"
            );
        }
    }
}

/// Every operation `session-stream-api` mounts, written independently of the
/// registry that this file then compares it against.
///
/// Deliberately hand-maintained: derive it from the same registry it guards and
/// the check compares the registry with itself. Every transport is covered,
/// including the finite session telemetry artifact routes. The retired
/// `regional-session-api` name once silently emptied this check, so the serving
/// artifact boundary remains explicit here.
const SESSION_STREAM_MOUNTS: &[&str] = &[
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
];

#[test]
fn actual_mounts_are_explicit_and_do_not_pollute_the_wire_bundle() {
    let root = repo_root();
    let tree = generate_to_memory(&root).expect("generation");
    let routes: serde_json::Value = serde_json::from_slice(
        tree.bytes("api/generated/registries/routes.json")
            .expect("route registry"),
    )
    .expect("route registry json");
    let rows = routes["routes"].as_array().expect("route rows");
    let stream_api: BTreeSet<_> = rows
        .iter()
        .filter(|route| route["servedArtifact"] == "session-api")
        .map(|route| route["operationId"].as_str().expect("operation id"))
        .collect();
    assert_eq!(
        stream_api,
        SESSION_STREAM_MOUNTS
            .iter()
            .copied()
            .collect::<BTreeSet<_>>(),
    );
    assert!(
        !rows
            .iter()
            .any(|route| route["servedArtifact"] == "session-stream-api"),
        "the launch artifact is `session-api`; the retired split name returned"
    );

    let bundle: serde_json::Value =
        serde_json::from_slice(tree.bytes("api/generated/bundle.json").expect("bundle"))
            .expect("bundle json");
    for plane in ["central", "regional"] {
        for route in bundle["planes"][plane]["operations"]
            .as_array()
            .expect("bundle route rows")
        {
            assert!(route.get("servingArtifact").is_none());
            assert!(route.get("servedArtifact").is_none());
        }
    }
}

/// The five projections of one deferral agree, or the suite fails.
///
/// Derivation comes first — every marker below is computed from the same
/// `deferred_reason` in one IR, in one pass — and this is the assertion that a
/// future emitter change cannot quietly drop one of them. Note what is *not*
/// asserted: no published artifact carries the ledger's prose. The reasons are
/// engineering notes written for engineers, and a stale one published to a
/// customer is worse than no sentence at all.
#[test]
fn deferred_operations_are_marked_in_every_published_artifact() {
    let root = repo_root();
    let tree = generate_to_memory(&root).expect("generation");
    let bundle: serde_json::Value =
        serde_json::from_slice(tree.bytes("api/generated/bundle.json").expect("bundle"))
            .expect("bundle json");
    let registry: serde_json::Value = serde_json::from_slice(
        tree.bytes("api/generated/registries/routes.json")
            .expect("route registry"),
    )
    .expect("route registry json");
    let deferred_in_registry: BTreeSet<&str> = registry["routes"]
        .as_array()
        .expect("route rows")
        .iter()
        .filter(|route| route.get("deferredReason").is_some())
        .map(|route| route["operationId"].as_str().expect("operation id"))
        .collect();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for plane in ["central", "regional"] {
        let document: serde_json::Value = serde_json::from_slice(
            tree.bytes(&format!("api/generated/openapi/aex-{plane}.json"))
                .expect("plane document"),
        )
        .expect("plane json");
        let bundle_rows: BTreeMap<&str, &serde_json::Value> = bundle["planes"][plane]["operations"]
            .as_array()
            .expect("bundle route rows")
            .iter()
            .map(|route| (route["operationId"].as_str().expect("operation id"), route))
            .collect();
        for (_, item) in document["paths"].as_object().expect("paths") {
            for (_, operation) in item.as_object().expect("path item") {
                let id = operation["operationId"].as_str().expect("operation id");
                let marked = operation.get("x-aex-deferred").is_some();
                let refuses = operation["responses"].get("501").is_some();
                let row = bundle_rows[id];
                let in_bundle = row.get("deferred").is_some();
                let in_registry = deferred_in_registry.contains(id);
                let declares = row["errors"]
                    .as_array()
                    .expect("declared errors")
                    .iter()
                    .any(|code| code == "not_implemented");
                assert_eq!(
                    [marked, refuses, in_bundle, in_registry, declares]
                        .iter()
                        .filter(|flag| **flag)
                        .count(),
                    if marked { 5 } else { 0 },
                    "`{id}` is marked in some published artifacts and not others: \
                     x-aex-deferred={marked} 501={refuses} bundle={in_bundle} \
                     registry={in_registry} declares={declares}"
                );
                assert_eq!(
                    operation.get("x-aex-deferred"),
                    if marked {
                        Some(&serde_json::Value::Bool(true))
                    } else {
                        None
                    },
                    "`{id}` publishes something other than a bare marker"
                );
                if marked {
                    seen.insert(id.to_owned());
                }
            }
        }
    }
    assert_eq!(
        seen,
        deferred_in_registry
            .iter()
            .map(|id| (*id).to_owned())
            .collect::<BTreeSet<String>>()
    );
}

#[test]
fn malformed_serving_artifacts_are_rejected_at_the_authored_boundary() {
    let temp = tempfile::tempdir().expect("temporary contract root");
    copy_authored_contract(temp.path());
    let path = temp.path().join("api/schemas/registries/routes-meta.yaml");
    let authored = std::fs::read_to_string(&path).expect("routes metadata");
    let text = replace_exactly_once(
        &authored,
        "control-api: central",
        "central--control-api: central",
        "malformed serving artifact",
    );
    std::fs::write(path, text).expect("mutate fixture metadata");
    let error = load::load(temp.path()).expect_err("malformed artifact must fail");
    assert!(error.to_string().contains("malformed serving artifact"));
}

#[test]
fn cross_plane_planned_owners_are_rejected() {
    let temp = tempfile::tempdir().expect("temporary contract root");
    copy_authored_contract(temp.path());
    let path = temp.path().join("api/schemas/registries/routes-meta.yaml");
    let authored = std::fs::read_to_string(&path).expect("routes metadata");
    let text = replace_exactly_once(
        &authored,
        "control-api: central",
        "control-api: regional",
        "cross-plane planned owner",
    );
    std::fs::write(path, text).expect("mutate fixture metadata");
    let error = load::load(temp.path()).expect_err("cross-plane owner must fail");
    assert!(error.to_string().contains("is on `central`"));
    assert!(error.to_string().contains("declared on `regional`"));
}

#[test]
fn cross_plane_actual_owners_are_rejected() {
    let temp = tempfile::tempdir().expect("temporary contract root");
    copy_authored_contract(temp.path());
    let path = temp.path().join("api/schemas/registries/routes-meta.yaml");
    let authored = std::fs::read_to_string(&path).expect("routes metadata");
    let without_central_owner = replace_exactly_once(
        &authored,
        "    - auth_config_get\n    - api_key_create\n",
        "    - auth_config_get\n",
        "remove the central actual owner",
    );
    let text = replace_exactly_once(
        &without_central_owner,
        "  session-api:\n",
        "  session-api:\n    - api_key_create\n",
        "add the regional actual owner",
    );
    std::fs::write(path, text).expect("mutate fixture metadata");
    let error = load::load(temp.path()).expect_err("cross-plane actual owner must fail");
    assert!(error.to_string().contains("operation is on `central`"));
    assert!(error.to_string().contains("declared on `regional`"));
}

#[test]
fn one_operation_cannot_be_both_served_and_deferred() {
    let temp = tempfile::tempdir().expect("temporary contract root");
    copy_authored_contract(temp.path());
    let path = temp.path().join("api/schemas/registries/routes-meta.yaml");
    let authored = std::fs::read_to_string(&path).expect("routes metadata");
    let mut document: serde_json::Value =
        serde_norway::from_str(&authored).expect("routes metadata parses");
    document["deferredOperations"] = serde_json::json!({
        "api_key_create": "contradictory state"
    });
    let text = serde_norway::to_string(&document).expect("mutated routes metadata encodes");
    std::fs::write(path, text).expect("mutate fixture metadata");
    let error = load::load(temp.path()).expect_err("conflicting route state must fail");
    assert!(
        error
            .to_string()
            .contains("both served and explicitly deferred")
    );
}

#[test]
fn every_published_schema_is_reachable_and_self_describing() {
    let root = repo_root();
    let tree = generate_to_memory(&root).expect("generation");
    let ir = load::load(&root).expect("load");
    for id in ir.schemas.keys() {
        let path = format!("api/generated/schemas/{id}.json");
        let bytes = tree
            .bytes(&path)
            .unwrap_or_else(|| panic!("`{id}` has no published schema"));
        let document: serde_json::Value = serde_json::from_slice(bytes).expect("schema json");
        assert_eq!(
            document["$id"],
            serde_json::Value::from(format!("https://schemas.aex.dev/v1/{id}.json"))
        );
        assert_eq!(
            document["$schema"],
            serde_json::Value::from("https://json-schema.org/draft/2020-12/schema")
        );
        assert!(
            document["description"]
                .as_str()
                .is_some_and(|d| !d.is_empty()),
            "`{id}` has no description"
        );
        if document["type"] == "object" {
            assert_eq!(
                document["additionalProperties"],
                serde_json::Value::Bool(false),
                "`{id}` accepts unknown members"
            );
        }
    }
}

#[test]
fn a_bundle_classifies_clean_against_itself() {
    let root = repo_root();
    let tree = generate_to_memory(&root).expect("generation");
    let bundle: serde_json::Value =
        serde_json::from_slice(tree.bytes("api/generated/bundle.json").expect("bundle"))
            .expect("bundle json");
    assert!(
        classify(&bundle, &bundle).is_empty(),
        "a bundle must not differ from itself"
    );
}

#[test]
fn the_classifier_reports_each_row_of_the_evolution_table() {
    let root = repo_root();
    let tree = generate_to_memory(&root).expect("generation");
    let base: serde_json::Value =
        serde_json::from_slice(tree.bytes("api/generated/bundle.json").expect("bundle"))
            .expect("bundle json");

    // A removed operation is breaking.
    let mut head = base.clone();
    let operations = head["planes"]["central"]["operations"]
        .as_array_mut()
        .expect("operations");
    let removed = operations.remove(0);
    let changes = classify(&base, &head);
    assert!(
        changes.iter().any(|change| {
            change.classification == Classification::Breaking
                && change.detail == "operation removed"
        }),
        "removing `{}` was not classified breaking",
        removed["operationId"]
    );

    // A new operation is materially compatible: the server ships first.
    let changes = classify(&head, &base);
    assert!(
        changes.iter().any(|change| {
            change.classification == Classification::MateriallyCompatible
                && change.detail == "new operation"
        }),
        "adding an operation was not classified materially compatible"
    );

    // A summary-only edit is documentation.
    let mut head = base.clone();
    head["planes"]["central"]["operations"][0]["summary"] =
        serde_json::Value::from("a different sentence.");
    let changes = classify(&base, &head);
    assert_eq!(changes.len(), 1, "{changes:#?}");
    assert_eq!(changes[0].classification, Classification::DocumentationOnly);

    // A narrowed `safeRetry` is breaking; a widened one is not.
    let mut head = base.clone();
    let operation = head["planes"]["central"]["operations"]
        .as_array_mut()
        .expect("central operations")
        .iter_mut()
        .find(|operation| operation["operationId"] == "api_keys_list")
        .expect("a safe-retry central operation");
    assert_eq!(operation["safeRetry"], serde_json::Value::Bool(true));
    operation["safeRetry"] = serde_json::Value::Bool(false);
    let changes = classify(&base, &head);
    assert!(
        changes
            .iter()
            .any(|change| change.classification == Classification::Breaking),
        "narrowing safeRetry was not classified breaking"
    );

    // A brand new error code is additive; removing one is breaking.
    let mut head = base.clone();
    let errors = head["registries"]["errors"]["errors"]
        .as_array_mut()
        .expect("errors");
    errors.push(serde_json::json!({
        "code": "a_new_code", "status": 400, "retryable": false,
        "class": "validation", "precedenceStage": "commit",
        "message": "new", "remedy": null,
    }));
    let changes = classify(&base, &head);
    assert!(
        changes
            .iter()
            .any(|change| change.classification == Classification::AdditiveResponse),
        "a new error code was not classified additive"
    );
    let changes = classify(&head, &base);
    assert!(
        changes
            .iter()
            .any(|change| change.classification == Classification::Breaking),
        "removing an error code was not classified breaking"
    );

    // A route landing is good news, and the paired disappearance of
    // `not_implemented` is its expected consequence, not a second finding.
    let (deferred_base, deferred_head) = deferral_pair(&base);
    let changes = classify(&deferred_base, &deferred_head);
    assert_eq!(
        changes
            .iter()
            .map(|change| (change.classification, change.detail.as_str()))
            .collect::<Vec<_>>(),
        vec![(
            Classification::MateriallyCompatible,
            "deferred operation is now served"
        )],
        "{changes:#?}"
    );

    // Withdrawing a served route into the ledger genuinely breaks callers.
    let changes = classify(&deferred_head, &deferred_base);
    assert_eq!(
        changes
            .iter()
            .map(|change| (change.classification, change.detail.as_str()))
            .collect::<Vec<_>>(),
        vec![(Classification::Breaking, "served operation is now deferred")],
        "{changes:#?}"
    );
}

/// One bundle whose first regional operation is deferred, and the same bundle
/// with that operation landed.
fn deferral_pair(base: &serde_json::Value) -> (serde_json::Value, serde_json::Value) {
    let mut deferred = base.clone();
    let mut served = base.clone();
    for (document, mark) in [(&mut deferred, true), (&mut served, false)] {
        let operation = document["planes"]["regional"]["operations"]
            .as_array_mut()
            .expect("operations")
            .first_mut()
            .expect("at least one regional operation");
        let row = operation.as_object_mut().expect("an operation object");
        let mut errors: Vec<serde_json::Value> = row["errors"]
            .as_array()
            .expect("declared errors")
            .iter()
            .filter(|code| *code != "not_implemented")
            .cloned()
            .collect();
        if mark {
            row.insert("deferred".to_owned(), serde_json::Value::Bool(true));
            errors.push(serde_json::Value::from("not_implemented"));
        } else {
            row.remove("deferred");
        }
        row.insert("errors".to_owned(), serde_json::Value::Array(errors));
    }
    (deferred, served)
}

#[test]
fn the_generator_carries_no_hidden_skips() {
    // Q-FLAKE: a suite that can quietly stop running is worse than a red one.
    let root = repo_root();
    let mut offenders = Vec::new();
    for directory in [
        "crates/aex-wire",
        "crates/aex-internal-contracts",
        "crates/aex-payment-contracts",
        "crates/aex-hands-protocol",
        "tools/aex-contract-gen",
    ] {
        let mut stack = vec![root.join(directory)];
        while let Some(current) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&current) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                // Assembled rather than written out, so this scanner does not
                // find itself and report a false positive.
                let attribute = concat!("#[", "ign", "ore");
                let env_skip = concat!("is_err() ", "{ return");
                for needle in [attribute, env_skip] {
                    if text.contains(needle) {
                        offenders.push(format!("{}: {needle}", path.display()));
                    }
                }
            }
        }
    }
    assert!(offenders.is_empty(), "skipped tests found:\n{offenders:#?}");
}
