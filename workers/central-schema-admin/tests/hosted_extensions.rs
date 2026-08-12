//! Hosted extension installation against real `PostgreSQL` machinery.

use aex_test_harness::containers::PostgresContainer;
use central_schema_admin::migration::native_migrator;
use central_schema_admin::runner::configure_outbox_wake;
use sqlx::{Connection as _, Executor as _, PgConnection};
use testcontainers::core::{CmdWaitFor, ExecCommand};

const AWS_COMMONS_CONTROL: &str =
    "comment = 'hosted fixture commons'\ndefault_version = '1.2'\nrelocatable = false\n";
const AWS_COMMONS_SQL: &str = "CREATE SCHEMA aws_commons;\n";
const AWS_LAMBDA_CONTROL: &str = "comment = 'hosted fixture lambda'\ndefault_version = '1.0'\nrelocatable = true\nrequires = 'aws_commons'\n";
const AWS_LAMBDA_SQL: &str = "CREATE SCHEMA aws_lambda;\nCREATE FUNCTION aws_lambda.invoke(function_name text, payload json, context text, invocation_type text)\nRETURNS TABLE(status_code integer)\nLANGUAGE sql\nAS 'SELECT CASE WHEN invocation_type = ''DryRun'' THEN 204 ELSE 202 END';\n";

struct Fixture {
    engine: PostgresContainer,
    url: String,
}

impl Fixture {
    async fn start() -> Self {
        let engine = PostgresContainer::start()
            .await
            .expect("the pinned PostgreSQL starts");
        let admin_url = engine.connection_url();
        let database = format!("aex_{}", uuid::Uuid::new_v4().simple());
        let mut admin = PgConnection::connect(&admin_url)
            .await
            .expect("the engine accepts a connection");
        let create: &'static str =
            Box::leak(format!("CREATE DATABASE {database}").into_boxed_str());
        admin
            .execute(create)
            .await
            .expect("the fixture database is created");
        let url = admin_url.rsplit_once('/').map_or_else(
            || admin_url.clone(),
            |(head, _)| format!("{head}/{database}"),
        );
        Self { engine, url }
    }

    async fn connect(&self) -> PgConnection {
        PgConnection::connect(&self.url)
            .await
            .expect("the fixture database accepts a connection")
    }

    /// Adds only the two Aurora extension packages to this disposable engine.
    ///
    /// Their catalog shape matches the hosted packages: both scripts create
    /// their named runtime schemas, `aws_commons` is non-relocatable without a
    /// control-file schema, and relocatable `aws_lambda` requires it.
    async fn install_hosted_extension_packages(&self) {
        self.engine
            .container()
            .exec(
                ExecCommand::new([
                    "sh",
                    "-c",
                    "set -eu; d=\"$(pg_config --sharedir)/extension\"; \
                     printf '%s' \"$AWS_COMMONS_CONTROL\" > \"$d/aws_commons.control\"; \
                     printf '%s' \"$AWS_COMMONS_SQL\" > \"$d/aws_commons--1.2.sql\"; \
                     printf '%s' \"$AWS_LAMBDA_CONTROL\" > \"$d/aws_lambda.control\"; \
                     printf '%s' \"$AWS_LAMBDA_SQL\" > \"$d/aws_lambda--1.0.sql\"",
                ])
                .with_env_vars([
                    ("AWS_COMMONS_CONTROL", AWS_COMMONS_CONTROL),
                    ("AWS_COMMONS_SQL", AWS_COMMONS_SQL),
                    ("AWS_LAMBDA_CONTROL", AWS_LAMBDA_CONTROL),
                    ("AWS_LAMBDA_SQL", AWS_LAMBDA_SQL),
                ])
                .with_cmd_ready_condition(CmdWaitFor::exit_code(0)),
            )
            .await
            .expect("the hosted extension packages are installed in the disposable engine");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn hosted_outbox_extensions_install_without_public_and_replay_exactly() {
    let fixture = Fixture::start().await;
    fixture.install_hosted_extension_packages().await;
    let mut connection = fixture.connect().await;
    native_migrator()
        .run(&mut connection)
        .await
        .expect("the bundle applies and drops public");

    let public_exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = 'public')")
            .fetch_one(&mut connection)
            .await
            .expect("the public-schema absence reads");
    assert!(!public_exists, "the hosted fixture matches the live plane");

    sqlx::query("CREATE SCHEMA aws_commons")
        .execute(&mut connection)
        .await
        .expect("the failed hosted attempt left an empty commons residue");

    let lambda_arn = "arn:aws:lambda:eu-west-1:111111111111:function:aex-dev-outbox-wake:live";
    configure_outbox_wake(&mut connection, lambda_arn)
        .await
        .expect("the empty residue is repaired before the hosted scripts create their schemas");
    configure_outbox_wake(&mut connection, lambda_arn)
        .await
        .expect("the exact hosted install is idempotent");

    let extensions: Vec<(String, String)> = sqlx::query_as(
        "SELECT extension.extname::text, namespace.nspname::text \
           FROM pg_extension AS extension \
           JOIN pg_namespace AS namespace ON namespace.oid = extension.extnamespace \
          WHERE extension.extname IN ('aws_commons', 'aws_lambda') \
          ORDER BY extension.extname",
    )
    .fetch_all(&mut connection)
    .await
    .expect("the extension catalog reads");
    assert_eq!(
        extensions,
        vec![
            ("aws_commons".to_owned(), "schema_admin".to_owned()),
            ("aws_lambda".to_owned(), "schema_admin".to_owned()),
        ]
    );

    let runtime_schema_owners: Vec<(String, String)> = sqlx::query_as(
        "SELECT namespace.nspname::text, extension.extname::text \
           FROM pg_namespace AS namespace \
           JOIN pg_depend AS dependency \
             ON dependency.classid = 'pg_namespace'::regclass \
            AND dependency.objid = namespace.oid \
            AND dependency.refclassid = 'pg_extension'::regclass \
           JOIN pg_extension AS extension ON extension.oid = dependency.refobjid \
          WHERE namespace.nspname IN ('aws_commons', 'aws_lambda') \
          ORDER BY namespace.nspname",
    )
    .fetch_all(&mut connection)
    .await
    .expect("the runtime schema ownership reads");
    assert_eq!(
        runtime_schema_owners,
        vec![
            ("aws_commons".to_owned(), "aws_commons".to_owned()),
            ("aws_lambda".to_owned(), "aws_lambda".to_owned()),
        ]
    );

    let invoke_schema: String = sqlx::query_scalar(
        "SELECT namespace.nspname::text \
           FROM pg_proc AS procedure \
           JOIN pg_namespace AS namespace ON namespace.oid = procedure.pronamespace \
          WHERE procedure.oid = 'aws_lambda.invoke(text,json,text,text)'::regprocedure",
    )
    .fetch_one(&mut connection)
    .await
    .expect("the installed invoke function schema reads");
    assert_eq!(invoke_schema, "aws_lambda");

    let public_schema_usage: i64 = sqlx::query_scalar(
        "SELECT count(*) \
           FROM pg_namespace AS namespace, \
                LATERAL aclexplode(coalesce(namespace.nspacl, acldefault('n', namespace.nspowner))) AS privilege \
          WHERE namespace.nspname IN ('aws_commons', 'aws_lambda') \
            AND privilege.grantee = 0 \
            AND privilege.privilege_type = 'USAGE'",
    )
    .fetch_one(&mut connection)
    .await
    .expect("the extension schema privileges read");
    assert_eq!(public_schema_usage, 0, "PUBLIC cannot enter either schema");

    let targets: Vec<String> =
        sqlx::query_scalar("SELECT lambda_arn FROM control.outbox_wake_target")
            .fetch_all(&mut connection)
            .await
            .expect("the configured target reads");
    assert_eq!(targets, vec![lambda_arn.to_owned()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn nonempty_unowned_extension_schema_is_never_dropped() {
    let fixture = Fixture::start().await;
    fixture.install_hosted_extension_packages().await;
    let mut connection = fixture.connect().await;
    native_migrator()
        .run(&mut connection)
        .await
        .expect("the bundle applies");
    sqlx::query("CREATE SCHEMA aws_commons")
        .execute(&mut connection)
        .await
        .expect("the unowned residue schema is created");
    sqlx::query("CREATE TABLE aws_commons.keep_me (id integer PRIMARY KEY)")
        .execute(&mut connection)
        .await
        .expect("the residue is made nonempty");

    let error = configure_outbox_wake(
        &mut connection,
        "arn:aws:lambda:eu-west-1:111111111111:function:aex-dev-outbox-wake:live",
    )
    .await
    .expect_err("a nonempty residue fails closed");
    assert!(
        error.to_string().contains("cannot drop schema aws_commons"),
        "unexpected error: {error}"
    );

    let residue_survived: bool =
        sqlx::query_scalar("SELECT to_regclass('aws_commons.keep_me') IS NOT NULL")
            .fetch_one(&mut connection)
            .await
            .expect("the residue survival reads");
    assert!(residue_survived, "the nonempty schema was not dropped");
    let installed_extensions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_extension WHERE extname IN ('aws_commons', 'aws_lambda')",
    )
    .fetch_one(&mut connection)
    .await
    .expect("the extension catalog reads");
    assert_eq!(installed_extensions, 0, "the failed install stayed atomic");
}
