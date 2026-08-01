//! The generator and `aex-wire` must canonicalize identically.
//!
//! The generator carries its own JCS implementation so a broken emission stays
//! repairable, which is only safe if the two agree byte for byte. That is what
//! this file asserts, on the same vectors both are held to.

use aex_contract_gen::jcs;
use aex_contract_gen::load::repo_root;

/// The vectors both implementations are checked against.
fn vectors() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({}),
        serde_json::json!([]),
        serde_json::json!(null),
        serde_json::json!({"b": 1, "a": 2}),
        serde_json::json!({"a": {"\u{e9}": 2, "e": 3}}),
        serde_json::json!({"z": [3, 1, 2], "n": null, "t": true, "s": "\u{fc}"}),
        serde_json::json!({"nested": {"deep": {"deeper": [{"x": 1}, {"y": 2}]}}}),
        serde_json::json!({"escapes": "quote \" backslash \\ newline \n tab \t"}),
        serde_json::json!({"unicode": "\u{1f600} \u{4e2d}\u{6587} \u{fffd}"}),
        serde_json::json!({"integral_float": 1.0, "fraction": 1.5, "negative": -0.25}),
        serde_json::json!({"big": 9_007_199_254_740_993_i64}),
    ]
}

#[test]
fn the_two_canonicalizers_agree_on_every_vector() {
    for value in vectors() {
        let from_tool = jcs::to_jcs_bytes(&value);
        let from_wire = aex_wire::to_jcs_bytes(&value).expect("aex-wire canonicalizes");
        assert_eq!(
            String::from_utf8(from_tool.clone()).expect("utf8"),
            String::from_utf8(from_wire).expect("utf8"),
            "the two canonicalizers disagree on {value}"
        );
    }
}

#[test]
fn member_order_is_utf8_byte_order_not_insertion_order() {
    let value: serde_json::Value =
        serde_json::from_str(r#"{"z":1,"B":2,"a":3,"A":4,"_":5}"#).expect("input");
    let text = String::from_utf8(jcs::to_jcs_bytes(&value)).expect("utf8");
    assert_eq!(text, r#"{"A":4,"B":2,"_":5,"a":3,"z":1}"#);
}

#[test]
fn the_committed_bundle_digest_is_the_digest_of_the_committed_bundle() {
    let root = repo_root();
    let bundle: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join("api/generated/bundle.json")).expect("read"),
    )
    .expect("bundle json");
    let lock: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join("api/generated/bundle.lock.json")).expect("read"),
    )
    .expect("lock json");
    assert_eq!(lock["contractDigest"], jcs::digest(&bundle));
}
