//! Migration bundle identity across the declared head.

mod common;

use aex_release_tool::migration::{
    Bundle, SchemaHead, build_bundle, reject_below_head_insert, verify_bundle,
    verify_deployed_prefix,
};

const BODY: &str =
    "-- aex-migration: tx=yes destructive=no phase=expand\nCREATE TABLE t (id bigint);\n";

fn fixture(files: &[(&str, &str)]) -> std::path::PathBuf {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    std::mem::forget(temp);
    std::fs::create_dir_all(root.join("migrations/central")).unwrap();
    for (name, body) in files {
        std::fs::write(root.join("migrations/central").join(name), body).unwrap();
    }
    std::fs::write(
        root.join("migrations/central/grants.toml"),
        "schema = \"aex.grants.v1\"\n",
    )
    .unwrap();
    root
}

fn head(bundle: &Bundle, central: &str) -> SchemaHead {
    SchemaHead {
        schema: "aex.schema-head.v1".to_owned(),
        central: central.to_owned(),
        central_bundle_digest: aex_release_tool::canon::to_string(bundle)
            .map(|text| aex_release_tool::canon::digest_bytes(text.as_bytes()))
            .expect("a canonical bundle"),
        regional_generation: 1,
        regional_bundle_digest: aex_release_tool::canon::digest_bytes(b"regional"),
    }
}

#[test]
fn a_sound_bundle_verifies_against_its_head() {
    let root = fixture(&[
        ("20260801000100_baseline.sql", BODY),
        ("20260801000200_expand.sql", BODY),
    ]);
    let bundle = build_bundle(&root).unwrap();
    verify_bundle(&bundle, &head(&bundle, "20260801000100")).expect("must verify");
    assert_eq!(bundle.head, "20260801000200");
    assert_eq!(bundle.files.len(), 2);
    assert!(bundle.files.iter().all(|file| file.tx));
    assert!(bundle.grants_digest.starts_with("sha256:"));
}

#[test]
fn a_bundle_head_below_the_declared_head_is_a_regression() {
    let root = fixture(&[("20260801000100_baseline.sql", BODY)]);
    let bundle = build_bundle(&root).unwrap();
    let err = verify_bundle(&bundle, &head(&bundle, "20260901000000")).unwrap_err();
    assert_eq!(err.exit.code(), 30);
    assert!(err.rules().contains(&"migration-head-regressed"));
}

#[test]
fn inserting_a_version_below_the_applied_head_fails() {
    let root = fixture(&[
        ("20260801000100_baseline.sql", BODY),
        ("20260801000300_third.sql", BODY),
    ]);
    let previous = build_bundle(&root).unwrap();
    std::fs::write(
        root.join("migrations/central/20260801000200_squeezed.sql"),
        BODY,
    )
    .unwrap();
    let current = build_bundle(&root).unwrap();
    let err = reject_below_head_insert(&previous, &current).unwrap_err();
    assert_eq!(err.exit.code(), 30);
    assert!(err.rules().contains(&"migration-below-head-insert"));
}

#[test]
fn editing_a_body_below_the_applied_head_fails() {
    let root = fixture(&[
        ("20260801000100_baseline.sql", BODY),
        ("20260801000300_third.sql", BODY),
    ]);
    let previous = build_bundle(&root).unwrap();
    std::fs::write(
        root.join("migrations/central/20260801000100_baseline.sql"),
        "-- aex-migration: tx=yes destructive=no phase=expand\nCREATE TABLE t (id int);\n",
    )
    .unwrap();
    let current = build_bundle(&root).unwrap();
    let err = reject_below_head_insert(&previous, &current).unwrap_err();
    assert!(err.rules().contains(&"migration-below-head-changed"));
}

#[test]
fn the_committed_deployed_prefix_matches_every_applied_migration_byte() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bundle = build_bundle(&root).expect("the source bundle builds");
    verify_deployed_prefix(&root, &bundle)
        .expect("versions through the committed deployed head remain byte-exact");
}

#[test]
fn bundle_construction_refuses_a_deliberate_deployed_prefix_mutation() {
    let root = fixture(&[("20260801000100_baseline.sql", BODY)]);
    let deployed = build_bundle(&root).expect("initial bundle");
    std::fs::write(
        root.join("migrations/central/deployed-prefix.lock.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema": "aex.deployed-migration-prefix.v1",
            "through": deployed.head,
            "files": deployed.files,
        }))
        .expect("prefix json"),
    )
    .expect("prefix lock");
    std::fs::write(
        root.join("migrations/central/20260801000100_baseline.sql"),
        "-- aex-migration: tx=yes destructive=no phase=expand\nCREATE TABLE t (id int);\n",
    )
    .expect("mutated migration");

    let error = build_bundle(&root).expect_err("an applied migration cannot change");
    assert!(error.rules().contains(&"migration-deployed-prefix-changed"));
}

#[test]
fn a_bundle_is_byte_stable_across_builds() {
    let root = fixture(&[("20260801000100_baseline.sql", BODY)]);
    let first = build_bundle(&root).unwrap();
    let second = build_bundle(&root).unwrap();
    assert_eq!(
        aex_release_tool::canon::to_string(&first).unwrap(),
        aex_release_tool::canon::to_string(&second).unwrap()
    );
}

#[test]
fn crlf_checkout_has_the_same_bundle_authority_as_canonical_lf() {
    let lf = fixture(&[("20260801000100_baseline.sql", BODY)]);
    let crlf_body = BODY.replace('\n', "\r\n");
    let crlf = fixture(&[("20260801000100_baseline.sql", &crlf_body)]);

    let crlf_bundle = build_bundle(&crlf).expect("CRLF checkout bundle");
    let lf_bundle = build_bundle(&lf).expect("LF checkout bundle");
    assert_eq!(
        aex_release_tool::canon::to_file_bytes(&crlf_bundle).expect("canonical CRLF bundle"),
        aex_release_tool::canon::to_file_bytes(&lf_bundle).expect("canonical LF bundle")
    );
}

#[test]
fn every_declared_failure_mode_reports_its_own_rule() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "no header",
            "CREATE TABLE t (id int);",
            "migration-header-missing",
        ),
        (
            "no-transaction disagreeing with tx",
            "-- aex-migration: tx=yes destructive=no phase=expand\n-- no-transaction\nX;",
            "migration-transaction-disagreement",
        ),
        (
            "non-transactional with no repair",
            "-- aex-migration: tx=no destructive=no phase=expand\nCREATE INDEX CONCURRENTLY i;",
            "migration-repair-missing",
        ),
        (
            "inline grant",
            "-- aex-migration: tx=yes destructive=no phase=expand\nGRANT SELECT ON t TO r;",
            "migration-inline-grant",
        ),
        (
            "unknown phase",
            "-- aex-migration: tx=yes destructive=no phase=whenever\nX;",
            "migration-header-malformed",
        ),
    ];
    for (name, body, rule) in cases {
        let root = fixture(&[("20260801000100_case.sql", body)]);
        let err = build_bundle(&root).unwrap_err_or_else(|| panic!("`{name}` should have failed"));
        assert!(
            err.rules().contains(rule),
            "`{name}` reported {:?}, expected `{rule}`",
            err.rules()
        );
    }
}

/// A tiny helper so the loop above reads as one assertion per case.
trait UnwrapErrOrElse<T, E> {
    fn unwrap_err_or_else(self, f: impl FnOnce() -> E) -> E;
}

impl<T, E> UnwrapErrOrElse<T, E> for Result<T, E> {
    fn unwrap_err_or_else(self, f: impl FnOnce() -> E) -> E {
        match self {
            Ok(_) => f(),
            Err(err) => err,
        }
    }
}
