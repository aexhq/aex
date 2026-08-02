//! The private-path corpus.
//!
//! One path per permitted category, one per deny id, one unclassified, one
//! ambiguous, and one carrying a credential shape. The classifier is public and
//! tested here; the private repository only invokes it.

mod common;

use aex_release_tool::private_path::{Policy, Verdict, check, classify};

fn shipped_policy() -> Policy {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../release/policy/private-path-policy.json"),
    )
    .expect("the shipped private-path policy");
    serde_json::from_str(&text).expect("the shipped policy must parse")
}

/// One path per permitted category, in the shape the shipped policy globs
/// declare.
///
/// The composition paths carry the `infra/terraform/` prefix the policy pins.
/// They did not when the corpus was written, and the classifier reported every
/// one of them unclassified until the corpus caught up — which is exactly what
/// this corpus exists to catch, one layer up.
const ALLOWED: &[(&str, &str)] = &[
    (
        "infra/terraform/composition/roots/dev/main.tf",
        "environment-root",
    ),
    (
        "infra/terraform/composition/roots/dev/eu-west-1/main.tf",
        "environment-root",
    ),
    (
        "infra/terraform/composition/roots/prd/terraform.tfvars.json",
        "environment-root",
    ),
    (
        "infra/terraform/composition/roots/prd/.terraform.lock.hcl",
        "environment-root",
    ),
    (
        "infra/terraform/composition/dev/binding.json",
        "composition",
    ),
    (
        "infra/terraform/composition/prd/binding.json",
        "composition",
    ),
    (
        "infra/terraform/composition/dev/eu-west-1/binding.json",
        "composition",
    ),
    (
        "infra/terraform/composition/prd/config.units.json",
        "composition",
    ),
    (
        "infra/terraform/composition/dev/secrets.dev.json",
        "secret-reference",
    ),
    (
        "infra/terraform/composition/prd/secrets.prd.json",
        "secret-reference",
    ),
    ("vault/keys.sops.json", "secret-reference"),
    ("vault/backup.age", "secret-reference"),
    (
        "infra/terraform/composition/README.md",
        "environment-documentation",
    ),
    (
        "infra/terraform/composition/roots/prd/README.md",
        "environment-documentation",
    ),
    (
        "infra/terraform/composition/dev/eu-west-1/README.md",
        "environment-documentation",
    ),
    ("business-data/price-book.json", "business-data"),
    ("business-data/tax/rates.csv", "business-data"),
    ("business-data/risk/thresholds.toml", "business-data"),
    ("operations/runbooks/deploy.md", "operations-record"),
    ("operations/incidents/2026-07-31.md", "operations-record"),
    ("operations/decisions/adr-0001.md", "operations-record"),
    ("operations/records/quota.json", "operations-record"),
    ("operations/reviews/postmortem.md", "operations-record"),
    ("operations/contacts/oncall.md", "private-contact"),
    ("operations/contacts/vendors.json", "private-contact"),
    ("operations/escalation/tier1.md", "private-contact"),
    ("operations/escalation/paging.json", "private-contact"),
];

const DENIED: &[(&str, &str)] = &[
    ("tools/helper/src/main.rs", "service-implementation"),
    ("scripts/deploy.ts", "service-implementation"),
    ("dashboard/panel.tsx", "service-implementation"),
    ("scripts/legacy.mjs", "service-implementation"),
    ("scripts/report.py", "service-implementation"),
    ("db/0001_init.sql", "database-migration"),
    ("contracts/openapi/session.yaml", "private-contract"),
    ("contracts/schemas/session.json", "private-contract"),
    ("wire/hands.proto", "private-contract"),
    ("suites/tests/smoke.md", "correctness-test"),
    ("suites/session.spec.json", "correctness-test"),
    ("modules/queue/tests/queue.tftest.hcl", "correctness-test"),
];

#[test]
fn every_permitted_category_accepts_exactly_its_own_paths() {
    let policy = shipped_policy();
    for (path, expected) in ALLOWED {
        match classify(&policy, path) {
            Verdict::Allowed { category } => {
                assert_eq!(&category, expected, "for `{path}`");
            }
            other => panic!("`{path}` classified as {other:?}, expected `{expected}`"),
        }
    }
}

#[test]
fn every_deny_class_rejects_its_representative_path() {
    let policy = shipped_policy();
    for (path, expected) in DENIED {
        match classify(&policy, path) {
            Verdict::Denied { deny } => assert_eq!(&deny, expected, "for `{path}`"),
            other => panic!("`{path}` classified as {other:?}, expected a `{expected}` denial"),
        }
    }
}

#[test]
fn no_permitted_path_is_ambiguous_across_two_categories() {
    // Ambiguity is a policy defect, not a runtime condition to handle: two
    // categories with different assertions cannot both apply to one file.
    let policy = shipped_policy();
    for (path, _) in ALLOWED {
        assert!(
            !matches!(classify(&policy, path), Verdict::Ambiguous { .. }),
            "`{path}` matches more than one category"
        );
    }
}

#[test]
fn an_unlisted_path_is_unclassified_rather_than_allowed() {
    let policy = shipped_policy();
    for path in [
        "scratch/notes.txt",
        "composition/local/binding.json",
        "assets/logo.png",
    ] {
        assert_eq!(
            classify(&policy, path),
            Verdict::Unclassified,
            "`{path}` should be unclassified"
        );
    }
}

#[test]
fn an_ambiguous_path_fails_rather_than_picking_the_first_match() {
    let policy: Policy = serde_json::from_str(
        r#"{
          "schema": "aex.private-path-policy.v1",
          "version": 1,
          "categories": [
            { "id": "first",  "globs": ["notes/**/*.json"], "assertions": [] },
            { "id": "second", "globs": ["**/*.json"], "assertions": [] }
          ],
          "deny": []
        }"#,
    )
    .unwrap();
    match classify(&policy, "notes/a.json") {
        Verdict::Ambiguous { categories } => assert_eq!(categories, vec!["first", "second"]),
        other => panic!("expected ambiguity, got {other:?}"),
    }
}

#[test]
fn the_check_reports_every_offending_path_at_once() {
    let policy = shipped_policy();
    let paths: Vec<String> = ["scratch/notes.txt", "db/0001_init.sql", "assets/logo.png"]
        .iter()
        .map(|path| (*path).to_owned())
        .collect();
    let err = check(&policy, &paths, |_| None).unwrap_err();
    assert_eq!(err.exit.code(), 60);
    assert_eq!(err.violations.len(), 3);
    assert!(err.rules().contains(&"private-path-denied"));
    assert!(err.rules().contains(&"private-path-unclassified"));
}

#[test]
fn a_plaintext_credential_in_an_allowed_file_fails_without_echoing_the_value() {
    let policy = shipped_policy();
    let paths = vec!["operations/runbooks/deploy.md".to_owned()];
    let secret = "AKIAIOSFODNN7EXAMPLE";
    let err = check(&policy, &paths, |_| {
        Some(format!("Run with AWS_ACCESS_KEY_ID={secret}\n"))
    })
    .unwrap_err();
    assert_eq!(err.exit.code(), 60);
    assert!(err.rules().contains(&"private-path-plaintext-secret"));
    for violation in &err.violations {
        assert!(
            !violation.detail.contains(secret),
            "the failure message must name the location, never the value"
        );
    }
}

#[test]
fn a_clean_allowed_corpus_passes() {
    let policy = shipped_policy();
    let paths: Vec<String> = ALLOWED.iter().map(|(path, _)| (*path).to_owned()).collect();
    check(&policy, &paths, |_| {
        Some("no credentials here\n".to_owned())
    })
    .expect("a clean private repository classifies cleanly");
}

#[test]
fn the_corpus_covers_every_category_and_deny_id_in_the_shipped_policy() {
    let policy = shipped_policy();
    for category in &policy.categories {
        assert!(
            ALLOWED.iter().any(|(_, id)| *id == category.id),
            "category `{}` has no corpus entry",
            category.id
        );
    }
    for deny in &policy.deny {
        if deny.globs.is_empty() {
            // Predicate-only rules are content checks, not path checks; they
            // are recorded as owed rather than silently treated as covered.
            continue;
        }
        assert!(
            DENIED.iter().any(|(_, id)| *id == deny.id),
            "deny rule `{}` has no corpus entry",
            deny.id
        );
    }
}
