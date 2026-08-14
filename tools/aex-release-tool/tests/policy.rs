//! Source and plan policies: Terraform packages no code, images are COPY-only,
//! and every workflow runs on a runner that exists.

mod common;

use aex_release_tool::policy::{
    PlanPolicy, check_change_isolation, check_plan, lint_workflow_text, lint_workflows,
    scan_dockerfile, scan_terraform, scan_terraform_text, summarize_plan,
};

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root")
}

// --- Terraform source ---------------------------------------------------------

#[test]
fn the_shipped_terraform_tree_packages_no_code() {
    let root = repo_root();
    let scanned = scan_terraform(&root).unwrap_or_else(|err| {
        panic!("the shipped Terraform tree violates the source policy:\n{err}")
    });
    assert!(
        scanned > 0,
        "the scanner found no .tf file at all, so it proved nothing"
    );
}

#[test]
fn every_forbidden_terraform_construct_is_rejected() {
    let cases: &[(&str, &str)] = &[
        (
            "data \"archive_file\" \"lambda\" {\n  type = \"zip\"\n}\n",
            "terraform-packages-code",
        ),
        (
            "data \"external\" \"build\" {\n  program = [\"bash\"]\n}\n",
            "terraform-packages-code",
        ),
        (
            "data \"http\" \"fetch\" {\n  url = \"https://example.invalid\"\n}\n",
            "terraform-packages-code",
        ),
        (
            "resource \"null_resource\" \"build\" {}\n",
            "terraform-packages-code",
        ),
        (
            "resource \"aws_instance\" \"x\" {\n  provisioner \"local-exec\" {}\n}\n",
            "terraform-packages-code",
        ),
        (
            "resource \"aws_lambda_invocation\" \"x\" {}\n",
            "terraform-invokes-code",
        ),
        (
            "resource \"docker_image\" \"x\" {}\n",
            "terraform-packages-code",
        ),
        (
            "resource \"aws_lambda_function\" \"x\" {\n  source_code_hash = filebase64sha256(\"a.zip\")\n}\n",
            "terraform-hashes-local-build",
        ),
    ];
    for (source, rule) in cases {
        let violations = scan_terraform_text("infra/modules/x/main.tf", source);
        assert!(
            violations.iter().any(|v| &v.rule == rule),
            "`{source}` reported {:?}, expected `{rule}`",
            violations.iter().map(|v| &v.rule).collect::<Vec<_>>()
        );
    }
}

#[test]
fn a_lambda_function_that_names_a_local_file_is_rejected() {
    let violations = scan_terraform_text(
        "infra/modules/lambda-function/main.tf",
        "resource \"aws_lambda_function\" \"this\" {\n  filename = \"build/bootstrap.zip\"\n}\n",
    );
    assert!(
        violations
            .iter()
            .any(|v| v.rule == "terraform-packages-code")
    );
}

#[test]
fn a_module_that_reads_a_file_is_rejected_and_a_root_is_not() {
    let source = "locals {\n  tables = jsondecode(file(\"../../tables.json\"))\n}\n";
    assert!(
        scan_terraform_text("infra/modules/regional-dynamodb-tables/main.tf", source)
            .iter()
            .any(|v| v.rule == "terraform-module-reads-file")
    );
    assert!(
        scan_terraform_text("infra/examples/region-application/main.tf", source)
            .iter()
            .all(|v| v.rule != "terraform-module-reads-file"),
        "only a root reads the two checked-in generated bundles"
    );
}

#[test]
fn a_comment_is_not_a_violation() {
    let violations = scan_terraform_text(
        "infra/modules/x/main.tf",
        "# never use a null_resource here\n// and no local-exec either\n",
    );
    assert!(violations.is_empty(), "{violations:?}");
}

// --- Dockerfile ----------------------------------------------------------------

#[test]
fn a_copy_only_image_over_a_digest_pinned_base_passes() {
    let dockerfile = format!(
        "FROM gcr.io/distroless/cc-debian12@sha256:{}\nCOPY bootstrap /app/bootstrap\n\
         USER 65532:65532\nEXPOSE 8080\nENTRYPOINT [\"/app/bootstrap\"]\n",
        "a".repeat(64)
    );
    assert!(scan_dockerfile("runtimes/brain-mux/Dockerfile", &dockerfile).is_empty());
}

#[test]
fn a_builder_stage_or_a_run_instruction_is_rejected() {
    let dockerfile = format!(
        "FROM rust@sha256:{a} AS builder\nRUN cargo build --release\n\
         FROM gcr.io/distroless/cc@sha256:{a}\nCOPY --from=builder /app /app\n",
        a = "b".repeat(64)
    );
    let violations = scan_dockerfile("runtimes/brain-mux/Dockerfile", &dockerfile);
    let rules: Vec<&str> = violations.iter().map(|v| v.rule.as_str()).collect();
    assert!(rules.contains(&"dockerfile-not-copy-only"));
    assert!(rules.contains(&"dockerfile-builder-stage"));
}

#[test]
fn an_unpinned_base_image_is_rejected() {
    let violations = scan_dockerfile(
        "runtimes/brain-mux/Dockerfile",
        "FROM gcr.io/distroless/cc-debian12:latest\nCOPY bootstrap /app/bootstrap\n",
    );
    assert!(
        violations
            .iter()
            .any(|v| v.rule == "dockerfile-base-not-pinned")
    );
}

#[test]
fn a_build_cache_mount_is_rejected() {
    let dockerfile = format!(
        "FROM gcr.io/distroless/cc@sha256:{}\nCOPY --mount=type=cache,target=/x a /a\n",
        "c".repeat(64)
    );
    assert!(
        scan_dockerfile("x/Dockerfile", &dockerfile)
            .iter()
            .any(|v| v.rule == "dockerfile-not-copy-only")
    );
}

// --- workflows -----------------------------------------------------------------

#[test]
fn every_shipped_workflow_passes_the_structural_gates() {
    let root = repo_root();
    let report = lint_workflows(&root)
        .unwrap_or_else(|err| panic!("the shipped workflows violate the lane policy:\n{err}"));
    assert!(
        report.files.len() >= 4,
        "expected at least the four lane classes, found {:?}",
        report.files
    );
    assert!(report.pins > 0, "no action pin was checked at all");
}

#[test]
fn protected_catalog_consumers_require_preflight_before_build() {
    // The snapshot verification runs before compilation in the same read-only
    // workflow, so the generated table a build compiles is the verified one.
    let workflow =
        std::fs::read_to_string(repo_root().join(".github/workflows/_compile-artifacts.yml"))
            .expect("compile workflow");
    let preflight = workflow
        .find("Verify the vendored models.dev snapshot")
        .expect("snapshot verification step");
    let build = workflow.find("- name: Build").expect("build step");
    assert!(
        preflight < build,
        "snapshot verification must run before compilation"
    );
    assert!(workflow.contains("matrix.name == 'brain-mux'"));
    assert!(workflow.contains("matrix.name == 'session-api'"));
    assert!(workflow.contains("env.update(recipe['env'])"));

    // The publishing half still owns `publish`, and must never regain the
    // recipe-driven compile: a job that can rebuild the bytes can publish bytes
    // no lane ever tested.
    let publish =
        std::fs::read_to_string(repo_root().join(".github/workflows/_build-artifacts.yml"))
            .expect("artifact workflow");
    assert!(publish.contains("inputs.publish"));
    assert!(
        !publish.contains("env.update(recipe['env'])"),
        "the publishing job must not run the recipe's build command"
    );
}

#[test]
fn a_self_hosted_runner_label_is_rejected() {
    // The label does not fail the run: the job queues for a runner that will
    // never appear and is discarded a day later with no error anywhere.
    let workflow = "jobs:\n  build:\n    runs-on: self-hosted\n    permissions:\n      \
                    contents: read\n    steps:\n      - run: echo hello\n";
    let (violations, _) = lint_workflow_text("pr.yml", workflow);
    let rules: Vec<&str> = violations.iter().map(|v| v.rule.as_str()).collect();
    assert!(rules.contains(&"workflow-banned-runner"));
    assert!(rules.contains(&"workflow-runner-not-github-hosted"));
}

#[test]
fn a_retired_runner_selector_variable_is_rejected() {
    let workflow = "jobs:\n  build:\n    runs-on: ${{ vars.AEX_CI_RUNNER }}\n    \
                    permissions:\n      contents: read\n    steps:\n      - run: echo hi\n";
    let (violations, _) = lint_workflow_text("pr.yml", workflow);
    assert!(
        violations
            .iter()
            .any(|v| v.rule == "workflow-banned-runner")
    );
}

#[test]
fn an_unpinned_or_uncommented_action_is_rejected() {
    let workflow = "jobs:\n  build:\n    runs-on: ubuntu-latest\n    permissions:\n      \
                    contents: read\n    steps:\n      - uses: actions/checkout@v6\n";
    let (violations, pins) = lint_workflow_text("pr.yml", workflow);
    assert_eq!(pins, 1);
    let rules: Vec<&str> = violations.iter().map(|v| v.rule.as_str()).collect();
    assert!(rules.contains(&"workflow-action-unpinned"));
    assert!(rules.contains(&"workflow-action-pin-uncommented"));
}

#[test]
fn the_line_scan_and_the_parser_must_agree_on_the_pin_count() {
    // A scanner that under-reports is worse than no scanner: it produces a
    // green result that means nothing.
    let workflow = format!(
        "jobs:\n  build:\n    runs-on: ubuntu-latest\n    permissions:\n      contents: read\n\
             steps:\n      - uses: actions/checkout@{sha}  # v6.1.0\n      \
         - uses: actions/setup-node@{sha}  # v6.5.0\n",
        sha = "d".repeat(40)
    );
    let (violations, pins) = lint_workflow_text("pr.yml", &workflow);
    assert_eq!(pins, 2);
    assert!(
        violations.is_empty(),
        "a correctly pinned workflow reported {violations:?}"
    );
}

#[test]
fn pull_request_target_is_rejected() {
    let workflow = "on:\n  pull_request_target:\njobs:\n  build:\n    runs-on: ubuntu-latest\n    \
                    permissions:\n      contents: read\n    steps:\n      - run: echo hi\n";
    let (violations, _) = lint_workflow_text("pr.yml", workflow);
    assert!(
        violations
            .iter()
            .any(|v| v.rule == "workflow-pull-request-target")
    );
}

#[test]
fn a_job_with_no_declared_permissions_is_rejected() {
    let workflow =
        "jobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo hi\n";
    let (violations, _) = lint_workflow_text("pr.yml", workflow);
    assert!(
        violations
            .iter()
            .any(|v| v.rule == "workflow-permissions-undeclared")
    );
}

#[test]
fn a_long_lived_npm_token_is_rejected() {
    let workflow = "jobs:\n  publish:\n    runs-on: ubuntu-latest\n    permissions:\n      \
                    contents: read\n    steps:\n      - run: npm publish\n        env:\n          \
                    NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}\n";
    let (violations, _) = lint_workflow_text("main.yml", workflow);
    assert!(violations.iter().any(|v| v.rule == "workflow-npm-token"));
}

#[test]
fn a_release_engine_step_that_builds_is_rejected() {
    let workflow = "jobs:\n  deploy:\n    runs-on: ubuntu-latest\n    permissions:\n      \
                    contents: read\n    steps:\n      - run: cargo build --release\n";
    let (violations, _) = lint_workflow_text("_release-engine.yml", workflow);
    assert!(
        violations.iter().any(|v| v.rule == "release-engine-builds"),
        "the release engine deploys bytes somebody else already built"
    );
    // The same step in a build lane is fine.
    let (violations, _) = lint_workflow_text("main.yml", workflow);
    assert!(violations.iter().all(|v| v.rule != "release-engine-builds"));
}

#[test]
fn an_unknown_concurrency_key_is_rejected() {
    let workflow = "concurrency:\n  group: x\n  cancel-in-progress: true\n  queue: fifo\n\
                    jobs:\n  build:\n    runs-on: ubuntu-latest\n    permissions:\n      \
                    contents: read\n    steps:\n      - run: echo hi\n";
    let (violations, _) = lint_workflow_text("pr.yml", workflow);
    assert!(
        violations
            .iter()
            .any(|v| v.rule == "workflow-concurrency-key")
    );
}

// --- Terraform plan ------------------------------------------------------------

fn shipped_plan_policy() -> PlanPolicy {
    let text = std::fs::read_to_string(repo_root().join("release/policy/terraform-policy.toml"))
        .expect("the shipped plan policy");
    toml::from_str(&text).expect("the shipped plan policy must parse")
}

fn plan(changes: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "resource_changes": changes })
}

#[test]
fn a_plan_that_would_create_a_null_resource_is_rejected() {
    let document = plan(&serde_json::json!([{
        "address": "module.app.null_resource.build",
        "change": { "actions": ["create"], "before": null, "after": {} }
    }]));
    let summary = summarize_plan(&document).unwrap();
    let err = check_plan(&summary, &shipped_plan_policy()).unwrap_err();
    assert_eq!(err.exit.code(), 70);
    assert!(err.rules().contains(&"plan-forbidden-resource"));
}

#[test]
fn a_plan_that_would_replace_durable_state_is_rejected() {
    let document = plan(&serde_json::json!([{
        "address": "module.regional.aws_dynamodb_table.session_authority",
        "change": { "actions": ["delete", "create"], "before": {}, "after": {} }
    }]));
    let summary = summarize_plan(&document).unwrap();
    let err = check_plan(&summary, &shipped_plan_policy()).unwrap_err();
    assert!(err.rules().contains(&"plan-destroys-protected-resource"));
}

#[test]
fn a_manifest_only_change_moves_exactly_the_artifact_attributes() {
    let document = plan(&serde_json::json!([{
        "address": "module.api.aws_lambda_function.this",
        "change": {
            "actions": ["update"],
            "before": { "s3_key": "lambda/api/aaa.zip", "memory_size": 1024 },
            "after": { "s3_key": "lambda/api/bbb.zip", "memory_size": 1024 }
        }
    }]));
    let summary = summarize_plan(&document).unwrap();
    check_change_isolation(&summary, true).expect("only the artifact key moved");
    let err = check_change_isolation(&summary, false).unwrap_err();
    assert!(
        err.rules().contains(&"plan-change-not-isolated"),
        "an infrastructure-only change must not move an artifact attribute"
    );
}

#[test]
fn an_infrastructure_only_change_moves_no_artifact_attribute() {
    let document = plan(&serde_json::json!([{
        "address": "module.api.aws_lambda_function.this",
        "change": {
            "actions": ["update"],
            "before": { "s3_key": "lambda/api/aaa.zip", "memory_size": 1024 },
            "after": { "s3_key": "lambda/api/aaa.zip", "memory_size": 2048 }
        }
    }]));
    let summary = summarize_plan(&document).unwrap();
    check_change_isolation(&summary, false).expect("only the shape moved");
    let err = check_change_isolation(&summary, true).unwrap_err();
    assert!(err.rules().contains(&"plan-change-not-isolated"));
}

#[test]
fn a_document_that_is_not_a_plan_is_rejected() {
    let err = summarize_plan(&serde_json::json!({ "hello": true })).unwrap_err();
    assert_eq!(err.exit.code(), 70);
    assert!(err.rules().contains(&"plan-unparseable"));
}
