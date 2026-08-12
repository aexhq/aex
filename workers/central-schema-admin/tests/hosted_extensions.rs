//! Hosted extension installation against real `PostgreSQL` machinery.

use aex_test_harness::containers::PostgresContainer;
use central_schema_admin::migration::native_migrator;
use central_schema_admin::runner::configure_outbox_wake;
use sqlx::{Connection as _, Executor as _, PgConnection};
use testcontainers::core::{CmdWaitFor, ExecCommand};

const AWS_COMMONS_CONTROL: &str =
    "comment = 'hosted fixture commons'\ndefault_version = '1.2'\nrelocatable = false\n";
const AWS_COMMONS_SQL: &str = "-- hosted fixture commons\n";
const AWS_LAMBDA_CONTROL: &str = "comment = 'hosted fixture lambda'\ndefault_version = '1.0'\nrelocatable = true\nrequires = 'aws_commons'\n";
const AWS_LAMBDA_SQL: &str = "CREATE FUNCTION invoke(function_name text, payload json, context text, invocation_type text)\nRETURNS TABLE(status_code integer)\nLANGUAGE sql\nAS 'SELECT CASE WHEN invocation_type = ''DryRun'' THEN 204 ELSE 202 END';\n";

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
    /// Their catalog shape matches the hosted packages: `aws_commons` is
    /// non-relocatable without a control-file schema, while relocatable
    /// `aws_lambda` requires it. The production installer still creates and
    /// installs both packages through `PostgreSQL` itself.
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

    let lambda_arn = "arn:aws:lambda:eu-west-1:111111111111:function:aex-dev-outbox-wake:live";
    configure_outbox_wake(&mut connection, lambda_arn)
        .await
        .expect("the hosted extensions install without a default target schema");
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
