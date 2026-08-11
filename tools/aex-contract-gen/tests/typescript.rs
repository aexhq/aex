//! The TypeScript binding is generated from the same IR as `aex-wire`.

use aex_contract_gen::generate_to_memory;
use aex_contract_gen::load::{load, repo_root};

/// One generated file, as text.
fn generated(path: &str) -> String {
    let tree = generate_to_memory(&repo_root()).expect("generate");
    String::from_utf8(
        tree.bytes(path)
            .unwrap_or_else(|| panic!("`{path}` is not generated"))
            .to_vec(),
    )
    .expect("generated TypeScript is UTF-8")
}

#[test]
fn every_schema_has_one_typescript_validator_and_type() {
    let root = repo_root();
    let ir = load(&root).expect("load contract IR");
    let tree = generate_to_memory(&root).expect("generate");
    let source = String::from_utf8(
        tree.bytes("packages/wire/src/generated/models.ts")
            .expect("TypeScript binding")
            .to_vec(),
    )
    .expect("generated TypeScript is UTF-8");

    for schema in ir.schemas.values() {
        assert!(
            source.contains(&format!("export const {}Schema", schema.id)),
            "{} has no generated validator",
            schema.id
        );
        assert!(
            source.contains(&format!("export type {} =", schema.id))
                || source.contains(&format!("export interface {} {{", schema.id)),
            "{} has no generated type",
            schema.id
        );
    }
}

#[test]
fn generated_typescript_keeps_the_closed_wire_invariants() {
    let tree = generate_to_memory(&repo_root()).expect("generate");
    let source = String::from_utf8(
        tree.bytes("packages/wire/src/generated/models.ts")
            .expect("TypeScript binding")
            .to_vec(),
    )
    .expect("generated TypeScript is UTF-8");

    assert!(
        source.contains("}).strict();"),
        "objects must reject unknown fields"
    );
    assert!(
        source.contains("z.discriminatedUnion"),
        "tagged unions must reject unknown arms"
    );
    assert!(
        source.contains("isUuidV7Body"),
        "identifier validation must check UUIDv7 bits, not just spelling"
    );
    assert!(
        source.contains("isId(\"workspace\", value)"),
        "schema identifier fields must share the UUIDv7 validator"
    );
    assert!(
        source.contains("UTF-8 bytes"),
        "text bounds must use wire byte lengths"
    );
}

#[test]
fn the_sdk_route_table_carries_every_operation_the_contract_declares() {
    let ir = load(&repo_root()).expect("load contract IR");
    let source = generated("packages/sdk/src/generated/routes.ts");

    for operation in ir.operations() {
        assert!(
            source.contains(&format!("  | \"{}\"\n", operation.id))
                || source.contains(&format!("  | \"{}\";\n", operation.id)),
            "{} is missing from the SDK `RouteId` union",
            operation.id
        );
        assert!(
            source.contains(&format!("    id: \"{}\",\n", operation.id)),
            "{} has no SDK route descriptor",
            operation.id
        );
        assert!(
            source.contains(&format!("    path: \"{}\",\n", operation.path)),
            "{} has no SDK path",
            operation.id
        );
        assert!(
            source.contains(&format!("    bodyClass: \"{}\",\n", operation.body_class)),
            "{} has no SDK request body class",
            operation.id
        );
    }
}

/// The published client offers a method for what the platform answers, and for
/// nothing else.
///
/// A method that can only return `501` puts the platform's answer in the client
/// and then lets it go stale: an installed SDK pins one contract digest
/// forever. `execute` stays total over `RouteId` — asserted below — so a
/// deferred operation is still callable by anyone who wants the real refusal.
#[test]
fn the_sdk_publishes_a_resource_method_for_every_served_operation_and_no_other() {
    let ir = load(&repo_root()).expect("load contract IR");
    let source = generated("packages/sdk/src/generated/resources.ts");

    for operation in ir.operations() {
        let method_generic = if operation.transport == "binary" {
            ""
        } else {
            "<T = unknown>"
        };
        let method = format!("  async {}{method_generic}(", camel(&operation.id));
        let published = source.contains(&method);
        // The frame streams are absent for the other half of the same reason:
        // the SDK transport decodes one JSON body, so a generated method over a
        // frame stream could not return one.
        let expected = operation.deferred_reason.is_none() && operation.transport != "ndjson";
        assert_eq!(
            published,
            expected,
            "`{}` is deferred={} transport={} but published={published}",
            operation.id,
            operation.deferred_reason.is_some(),
            operation.transport
        );
    }
}

#[test]
fn the_sdk_route_table_stays_total_over_every_operation_including_the_deferred() {
    let ir = load(&repo_root()).expect("load contract IR");
    let source = generated("packages/sdk/src/generated/routes.ts");
    let deferred = ir
        .operations()
        .iter()
        .filter(|operation| operation.deferred_reason.is_some())
        .count();
    assert!(deferred > 0, "the ledger is empty; this proves nothing");
    for operation in ir.operations() {
        let opening = format!("\n  {}: {{\n", operation.id);
        let start = source
            .find(&opening)
            .unwrap_or_else(|| panic!("{} left the SDK route table", operation.id));
        let row = &source[start..];
        let row = &row[..row.find("\n  },\n").expect("a closed descriptor")];
        assert!(
            row.contains(&format!(
                "    deferred: {},",
                operation.deferred_reason.is_some()
            )),
            "{} carries the wrong deferral flag",
            operation.id
        );
    }
}

/// `snake_case` to `camelCase`, matching the emitter.
fn camel(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut capitalize = false;
    for character in text.chars() {
        if character == '_' {
            capitalize = true;
        } else if capitalize {
            out.extend(character.to_uppercase());
            capitalize = false;
        } else {
            out.push(character);
        }
    }
    out
}

#[test]
fn the_sdk_error_vocabulary_carries_every_code_and_class() {
    let ir = load(&repo_root()).expect("load contract IR");
    let source = generated("packages/sdk/src/generated/errors.ts");

    for row in &ir.errors {
        assert!(
            source.contains(&format!(
                "    {}: {{ class: \"{}\" }},\n",
                row.code, row.class
            )),
            "`{}` has no SDK error metadata row",
            row.code
        );
    }
}

#[test]
fn the_generated_sdk_sources_import_nothing() {
    // `@aexhq/sdk` is published with an empty `dependencies` map. That is why
    // the route table is emitted into the package instead of imported from
    // `@aexhq/wire`, and an emitted `import` would quietly undo it.
    for path in [
        "packages/sdk/src/generated/routes.ts",
        "packages/sdk/src/generated/errors.ts",
    ] {
        let source = generated(path);
        assert!(
            !source.contains("import "),
            "`{path}` imports; the SDK must stay dependency-free"
        );
        assert!(
            source.starts_with("// @generated by aex-contract-gen; DO NOT EDIT.\n"),
            "`{path}` does not open with the repository's generated marker"
        );
    }
    // The resource surface names `RouteId`, so it does import — but only its own
    // sibling. A bare package specifier here would put a dependency back into a
    // package published with an empty `dependencies` map.
    let resources = generated("packages/sdk/src/generated/resources.ts");
    assert!(
        resources.starts_with("// @generated by aex-contract-gen; DO NOT EDIT.\n"),
        "the resource surface does not open with the generated marker"
    );
    for line in resources.lines().filter(|line| line.contains("from \"")) {
        assert!(
            line.contains("from \"./"),
            "`{line}` reaches outside the package"
        );
    }
}
