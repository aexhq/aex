//! The generated server-trait, dispatch and client surface.
//!
//! Coverage here is a floor, not a sample: every one of the operations the
//! contract declares must reach a server-trait method, a dispatch arm, a client
//! method and a request builder. A route that is authored but never mounted is
//! exactly the failure this suite exists to make loud.

use std::collections::{BTreeMap, BTreeSet};

use aex_contract_gen::generate_to_memory;
use aex_contract_gen::load::{load, repo_root};

/// The generated tree, as UTF-8 text keyed by workspace-relative path.
fn generated() -> BTreeMap<String, String> {
    let tree = generate_to_memory(&repo_root()).expect("generation");
    tree.file_names()
        .iter()
        .map(|name| {
            let bytes = tree.bytes(name).expect("named file has bytes");
            (
                (*name).to_owned(),
                String::from_utf8(bytes.to_vec()).expect("generated output is UTF-8"),
            )
        })
        .collect()
}

#[test]
fn every_operation_has_a_server_trait_method() {
    let tree = generated();
    let server = tree
        .get("crates/aex-wire/src/generated/server.rs")
        .expect("the server emitter produced no file");
    let ir = load(&repo_root()).expect("load");
    let mut missing = Vec::new();
    for operation in ir.operations() {
        if !server.contains(&format!("    fn {}(", operation.id)) {
            missing.push(operation.id.clone());
        }
    }
    assert!(
        missing.is_empty(),
        "{} operations have no server-trait method: {missing:#?}",
        missing.len()
    );
}

#[test]
fn every_operation_has_a_dispatch_arm() {
    let tree = generated();
    let server = tree
        .get("crates/aex-wire/src/generated/server.rs")
        .expect("the server emitter produced no file");
    let ir = load(&repo_root()).expect("load");
    let mut missing = Vec::new();
    for operation in ir.operations() {
        if !server.contains(&format!("RouteId::{} =>", operation.variant)) {
            missing.push(operation.id.clone());
        }
    }
    assert!(
        missing.is_empty(),
        "{} operations have no dispatch arm: {missing:#?}",
        missing.len()
    );
}

#[test]
fn every_operation_has_a_client_method_and_a_request_builder() {
    let tree = generated();
    let client = tree
        .get("crates/aex-wire/src/generated/client.rs")
        .expect("the client emitter produced no file");
    let ir = load(&repo_root()).expect("load");
    let mut missing = Vec::new();
    for operation in ir.operations() {
        if !client.contains(&format!("    pub async fn {}(", operation.id)) {
            missing.push(format!("{} (method)", operation.id));
        }
        if !client.contains(&format!("pub fn {}_request(", operation.id)) {
            missing.push(format!("{} (request builder)", operation.id));
        }
    }
    assert!(
        missing.is_empty(),
        "{} client gaps: {missing:#?}",
        missing.len()
    );
}

#[test]
fn every_route_group_declares_one_trait_and_one_dispatcher() {
    let tree = generated();
    let server = tree
        .get("crates/aex-wire/src/generated/server.rs")
        .expect("the server emitter produced no file");
    let ir = load(&repo_root()).expect("load");

    // A fragment stem that exists on both planes is disambiguated by plane; every
    // other stem stands alone. Two traits must never share a name.
    let mut stems: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for operation in ir.operations() {
        stems
            .entry(operation.fragment.as_str())
            .or_default()
            .insert(operation.plane.as_str());
    }
    assert!(!stems.is_empty(), "no fragments were loaded");
    for (stem, planes) in &stems {
        for plane in planes {
            let name = if planes.len() > 1 {
                format!("{plane}-{stem}")
            } else {
                (*stem).to_owned()
            };
            let pascal = aex_contract_gen::load::pascal_case(&name);
            assert!(
                server.contains(&format!("pub trait {pascal}Api:")),
                "no `{pascal}Api` trait for fragment `{stem}` on plane `{plane}`"
            );
            let snake = name.replace('-', "_");
            assert!(
                server.contains(&format!("pub async fn dispatch_{snake}<")),
                "no `dispatch_{snake}` for fragment `{stem}` on plane `{plane}`"
            );
        }
    }
}

#[test]
fn the_surface_corpus_carries_exactly_one_line_per_operation() {
    let tree = generated();
    let corpus = tree
        .get("conformance/routes/surface.jsonl")
        .expect("the surface corpus was not generated");
    let ir = load(&repo_root()).expect("load");

    let mut seen = BTreeSet::new();
    for line in corpus.lines() {
        let row: serde_json::Value = serde_json::from_str(line).expect("surface row is JSON");
        let id = row["operationId"]
            .as_str()
            .expect("every row names its operation")
            .to_owned();
        for key in [
            "routeId",
            "plane",
            "group",
            "trait",
            "method",
            "responseShape",
            "successStatus",
            "transport",
            "idempotency",
        ] {
            assert!(!row[key].is_null(), "`{id}` has no `{key}`");
        }
        assert!(seen.insert(id.clone()), "`{id}` appears twice");
    }
    let expected: BTreeSet<String> = ir
        .operations()
        .iter()
        .map(|operation| operation.id.clone())
        .collect();
    assert_eq!(seen, expected, "the surface corpus does not cover the table");
    assert_eq!(seen.len(), 146, "the pinned operation arity moved");
}
