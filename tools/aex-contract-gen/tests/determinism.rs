//! The determinism and drift gates.
//!
//! "Regenerate twice" is only meaningful if nothing in the pipeline reads
//! ambient state, so these tests generate wholly in memory and compare bytes.
//! `checked_in_output_matches_regeneration` is what makes a hand edit of a
//! generated file a red test rather than a silent divergence.

use std::collections::BTreeSet;

use aex_contract_gen::classify::{Classification, classify};
use aex_contract_gen::load::repo_root;
use aex_contract_gen::{emit, generate_to_memory, jcs, load};

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
fn the_operation_arity_matches_the_pinned_totals() {
    let ir = load::load(&repo_root()).expect("load");
    let central = ir
        .planes
        .iter()
        .find(|plane| plane.id == "central")
        .expect("central plane");
    let regional = ir
        .planes
        .iter()
        .find(|plane| plane.id == "regional")
        .expect("regional plane");
    assert_eq!(central.operations.len(), 27);
    assert_eq!(regional.operations.len(), 119);
    assert_eq!(ir.operations().len(), 146);

    let ids: BTreeSet<&str> = ir
        .operations()
        .iter()
        .map(|operation| operation.id.as_str())
        .collect();
    assert_eq!(ids.len(), 146, "operationIds are not globally unique");
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
    head["planes"]["central"]["operations"][0]["safeRetry"] = serde_json::Value::Bool(false);
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
