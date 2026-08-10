//! The offline half of the one-shot task: the plan receipt, the exit contract
//! and the refusals that happen before a credential is ever resolved.

use std::process::Command;

/// The invocation prefix every subcommand shares.
fn base() -> Vec<&'static str> {
    vec![
        "--database-host",
        "db.example",
        "--database-port",
        "5432",
        "--database-name",
        "aex",
        "--database-secret-arn",
        "arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex/schema-admin",
        "--tls-root-ca-path",
        "fixture.pem",
        "--plane",
        "dev",
        "--release",
        "rel_fixture",
    ]
}

/// Runs the artifact with `arguments` appended to the shared prefix.
fn run(arguments: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_central-schema-admin"));
    command.args(base());
    command.args(arguments);
    command.env_clear();
    command.output().expect("the schema admin artifact starts")
}

#[test]
fn the_local_plan_emits_the_bound_lock_and_the_linear_head() {
    let output = run(&["plan"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8 receipt");
    assert!(stdout.contains("\"bundleHead\":20260801001200"));
    assert!(stdout.contains("\"lockKey\":4703262552200136530"));
}

#[test]
fn a_stale_image_is_refused_by_the_expected_bundle_head_before_anything_else() {
    let output = run(&["migrate", "--expect-head", "20260801000400"]);
    assert_eq!(
        output.status.code(),
        Some(12),
        "a bundle head mismatch is exit 12: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("20260801001200"), "{stderr}");
}

#[test]
fn a_verify_against_the_wrong_bundle_head_never_opens_a_session() {
    let output = run(&["verify", "--expect-head", "20260801000100"]);
    assert_eq!(output.status.code(), Some(12));
}

#[test]
fn the_exit_contract_is_observable_from_outside_the_process() {
    // Asserted through the artifact rather than in the source: a code a release
    // script depends on must be observable from outside the process.
    assert_eq!(run(&["plan"]).status.code(), Some(0));
    assert_eq!(
        run(&["migrate", "--expect-head", "1"]).status.code(),
        Some(12)
    );
    assert_eq!(
        run(&["verify", "--expect-head", "1"]).status.code(),
        Some(12)
    );
}

#[test]
fn a_repair_of_a_transactional_migration_is_refused() {
    // Every committed migration is transactional, so no repair path exists and
    // asking for one is a precondition failure rather than an improvised fix.
    let output = run(&[
        "repair",
        "--migration",
        "20260801000500",
        "--confirm",
        "rpr_0000000000000000",
    ]);
    assert_eq!(output.status.code(), Some(13));
}

#[test]
fn a_backfill_of_an_unbundled_migration_is_refused_before_a_session() {
    let output = run(&["backfill", "--migration", "20990101000000"]);
    assert_eq!(output.status.code(), Some(12));
}

#[test]
fn the_committed_bundle_declares_nothing_destructive() {
    // A destructive bundle needs recorded backup evidence; the baseline must not
    // silently require an operator to pass one.
    let output = run(&["migrate", "--expect-head", "20260801001200"]);
    assert_ne!(
        output.status.code(),
        Some(15),
        "the baseline bundle is not destructive: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn the_committed_bundle_holds_no_checksum_exception_mechanism() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations/central");
    for forbidden in [
        "approved-checksum-transitions.toml",
        "checksum-exceptions.toml",
    ] {
        assert!(
            !directory.join(forbidden).exists(),
            "`{forbidden}` rewrites history and defeats the checksum"
        );
    }
}

#[test]
fn a_pepper_seed_with_an_impossible_version_or_no_reference_never_opens_a_session() {
    // Both are decided from argv, before a credential is resolved, so a
    // malformed release argument costs no connection and no lock.
    for arguments in [
        vec![
            "seed-pepper",
            "--pepper",
            "api-key",
            "--version",
            "0",
            "--secret-ref",
            "some-version-id",
        ],
        vec![
            "seed-pepper",
            "--pepper",
            "api-key",
            "--version",
            "2",
            "--secret-ref",
            "   ",
        ],
    ] {
        let output = run(&arguments);
        assert_eq!(
            output.status.code(),
            Some(13),
            "a refused precondition is exit 13: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn a_pepper_seed_can_only_name_one_of_the_four_rows_that_exist() {
    // `--schema`/`--purpose` would admit `identity`/`api_key`, which no `CHECK`
    // allows and which would fail at deploy time instead of at parse time.
    let output = run(&[
        "seed-pepper",
        "--pepper",
        "identity-api-key",
        "--version",
        "1",
        "--secret-ref",
        "some-version-id",
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("api-key"), "the four values are named: {stderr}");
}
