//! The TypeScript binding is generated from the same IR as `aex-wire`.

use aex_contract_gen::generate_to_memory;
use aex_contract_gen::load::{load, repo_root};

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
