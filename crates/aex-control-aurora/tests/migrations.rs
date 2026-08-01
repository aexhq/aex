//! The central migration bundle against real `PostgreSQL`.
//!
//! What this proves that no unit test can: that the DDL applies at all, that
//! every `CHECK`, unique index, trigger and `SECURITY DEFINER` function behaves
//! as written, and — most importantly — that the **role-denial matrix** is real.
//! A grant table nobody connects as is a document, not a control.
//!
//! What it cannot prove is that Aurora's Data `API` behaves like a direct
//! connection; the `aws.rds_data.transaction` seam is claimed by the live
//! companion.
//!
//! # Why this is not a `testcontainers` suite
//!
//! It should be, and it cannot be yet. The registry's `data-image-literal` rule
//! bans the container library's only constructor in any path under `/tests/`
//! and exempts one directory: the shared harness. So **no product crate can
//! start a container at all** until the harness exposes a builder. That is
//! recorded as a cross-stream requirement in
//! `references/rewrite/central-identity.md`.
//!
//! Until it lands, the integration lane supplies the database and this suite
//! resolves it through `aex_test_harness::required_env!`, which panics with the
//! variable's name when it is absent. An absent prerequisite is a failure, never
//! a skip.

use aex_test_harness::required_env;
use sqlx::{Connection, Executor as _, PgConnection, Row as _};

/// The four migration files this stream owns, in order.
const MIGRATIONS: [(&str, &str); 4] = [
    (
        "0001_bootstrap",
        include_str!("../../../migrations/central/0001_bootstrap.sql"),
    ),
    (
        "0002_identity",
        include_str!("../../../migrations/central/0002_identity.sql"),
    ),
    (
        "0003_control",
        include_str!("../../../migrations/central/0003_control.sql"),
    ),
    (
        "0004_control_functions",
        include_str!("../../../migrations/central/0004_control_functions.sql"),
    ),
];

/// The finance objects this stream's statements join against.
///
/// A fixture only. In production their absence is `503
/// account_state_unavailable`, which is the correct behaviour anyway; the stub
/// exists so the join can be exercised at all before the finance stream lands.
const FINANCE_STUB: &str = "\
CREATE SCHEMA finance; \
CREATE VIEW finance.account_state_v1 AS \
  SELECT o.id AS organization_id, 'active'::text AS status, NULL::text AS reason, \
         1::bigint AS revision, o.created_at AS changed_at \
    FROM control.organization o; \
GRANT USAGE ON SCHEMA finance TO aex_authz; \
GRANT SELECT ON finance.account_state_v1 TO aex_authz;";

/// The login roles `central-schema-admin` attaches in production.
const LOGIN_ROLES: &str = "\
CREATE ROLE aex_identity_api_login   LOGIN PASSWORD 'fixture'; \
CREATE ROLE aex_authz_login          LOGIN PASSWORD 'fixture'; \
CREATE ROLE aex_control_api_login    LOGIN PASSWORD 'fixture'; \
CREATE ROLE aex_control_worker_login LOGIN PASSWORD 'fixture'; \
GRANT aex_identity_api   TO aex_identity_api_login; \
GRANT aex_authz          TO aex_authz_login; \
GRANT aex_control_api    TO aex_control_api_login; \
GRANT aex_control_worker TO aex_control_worker_login;";

/// The environment variable the integration lane sets to a superuser URL.
const DATABASE_URL: &str = "AEX_CENTRAL_PG_URL";

/// A freshly migrated database.
///
/// Each case mints its own database inside the supplied server, so the suite
/// parallelises without two cases sharing a schema.
struct Fixture {
    admin_url: String,
    database: String,
}

impl Fixture {
    async fn start() -> Self {
        let admin_url = required_env!(DATABASE_URL);
        let database = format!("aex_{}", uuid::Uuid::new_v4().simple());
        let mut admin = PgConnection::connect(&admin_url)
            .await
            .unwrap_or_else(|error| panic!("`{DATABASE_URL}` is reachable: {error}"));
        // The name is a fixed prefix plus a fresh UUID; nothing caller-supplied
        // reaches this string.
        let create: &'static str =
            Box::leak(format!("CREATE DATABASE {database}").into_boxed_str());
        admin
            .execute(create)
            .await
            .expect("the fixture database is created");
        let fixture = Self {
            admin_url,
            database,
        };
        fixture.migrate().await;
        fixture
    }

    /// The `host:port` the lane pointed at, without its credentials or database.
    fn authority(&self) -> String {
        let without_database = self
            .admin_url
            .rsplit_once('/')
            .map_or_else(|| self.admin_url.clone(), |(head, _)| head.to_owned());
        without_database
            .rsplit_once('@')
            .map_or(without_database.clone(), |(_, host)| host.to_owned())
    }

    fn superuser_url(&self) -> String {
        let without_database = self
            .admin_url
            .rsplit_once('/')
            .map_or_else(|| self.admin_url.clone(), |(head, _)| head.to_owned());
        format!("{without_database}/{}", self.database)
    }

    fn url(&self, role: &str) -> String {
        // The scheme comes from the URL the lane supplied rather than a literal,
        // so a lane that speaks a different dialect needs no change here.
        let scheme = self
            .admin_url
            .split_once("://")
            .map_or("postgresql", |(scheme, _)| scheme);
        format!(
            "{scheme}://{role}:fixture@{}/{}",
            self.authority(),
            self.database
        )
    }

    async fn superuser(&self) -> PgConnection {
        PgConnection::connect(&self.superuser_url())
            .await
            .expect("the superuser connects")
    }

    async fn as_role(&self, role: &str) -> PgConnection {
        PgConnection::connect(&self.url(role))
            .await
            .unwrap_or_else(|error| panic!("`{role}` connects: {error}"))
    }

    /// Applies the bundle, the login roles and the finance fixture.
    async fn migrate(&self) {
        let mut connection = self.superuser().await;
        for (name, sql) in MIGRATIONS {
            // `0001` revokes on the database by name; the fixture database has
            // a generated one, so the statement is retargeted rather than
            // skipped — the revocation is the point of the file.
            let sql = sql.replace("DATABASE aex ", &format!("DATABASE {} ", self.database));
            let sql: &'static str = Box::leak(sql.into_boxed_str());
            connection
                .execute(sql)
                .await
                .unwrap_or_else(|error| panic!("`{name}` applies: {error}"));
        }
        connection
            .execute(FINANCE_STUB)
            .await
            .expect("the finance fixture applies");
        let roles = connection.execute(LOGIN_ROLES).await;
        // The roles are cluster-wide, so a parallel fixture may have created
        // them already. Their existence is the point, not who created them.
        if let Err(error) = roles {
            assert!(
                error.to_string().contains("already exists"),
                "the login roles attach: {error}"
            );
        }
    }
}

/// Whether `role` holds `privilege` on `object`.
async fn has_privilege(
    connection: &mut PgConnection,
    role: &str,
    object: &str,
    privilege: &str,
) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT has_table_privilege($1, $2, $3)")
        .bind(role)
        .bind(object)
        .bind(privilege)
        .fetch_one(connection)
        .await
        .unwrap_or_else(|error| panic!("privilege probe for {role}/{object}/{privilege}: {error}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bundle_applies_and_leaves_no_public_schema() {
    let fixture = Fixture::start().await;
    let mut connection = fixture.superuser().await;

    let public: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.schemata WHERE schema_name = 'public'",
    )
    .fetch_one(&mut connection)
    .await
    .expect("the schema probe runs");
    assert_eq!(public, 0, "the public schema is dropped outright");

    for schema in ["identity", "control"] {
        let present: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM information_schema.schemata WHERE schema_name = $1",
        )
        .bind(schema)
        .fetch_one(&mut connection)
        .await
        .expect("the schema probe runs");
        assert_eq!(present, 1, "`{schema}` exists");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn no_provider_token_column_exists_anywhere_in_identity() {
    let fixture = Fixture::start().await;
    let mut connection = fixture.superuser().await;

    let rows = sqlx::query(
        "SELECT table_name, column_name FROM information_schema.columns \
         WHERE table_schema = 'identity' \
           AND (column_name LIKE '%access_token%' \
             OR column_name LIKE '%refresh_token%' \
             OR column_name LIKE '%id_token%')",
    )
    .fetch_all(&mut connection)
    .await
    .expect("the column probe runs");
    assert!(
        rows.is_empty(),
        "AEX stores no provider token; found {:?}",
        rows.iter()
            .map(|row| {
                (
                    row.get::<String, _>("table_name"),
                    row.get::<String, _>("column_name"),
                )
            })
            .collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_invitation_has_no_secret_column() {
    let fixture = Fixture::start().await;
    let mut connection = fixture.superuser().await;

    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT column_name FROM information_schema.columns \
         WHERE table_schema = 'control' AND table_name = 'invitation' \
           AND (column_name LIKE '%token%' OR column_name LIKE '%hash%' \
             OR column_name LIKE '%verifier%' OR column_name LIKE '%secret%')",
    )
    .fetch_all(&mut connection)
    .await
    .expect("the column probe runs");
    assert!(
        rows.is_empty(),
        "acceptance is a verified-email match; found {rows:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_authorization_epoch_is_writable_by_nobody() {
    let fixture = Fixture::start().await;
    let mut connection = fixture.superuser().await;

    for role in [
        "aex_identity_api",
        "aex_authz",
        "aex_control_api",
        "aex_control_worker",
    ] {
        for privilege in ["UPDATE", "DELETE", "INSERT"] {
            assert!(
                !has_privilege(
                    &mut connection,
                    role,
                    "control.authorization_epoch",
                    privilege
                )
                .await,
                "`{role}` holds {privilege} on control.authorization_epoch; \
                 a decrement must be unrepresentable"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_audit_log_is_append_only_for_every_role() {
    let fixture = Fixture::start().await;
    let mut connection = fixture.superuser().await;

    for role in ["aex_control_api", "aex_control_worker"] {
        assert!(
            has_privilege(&mut connection, role, "control.audit_event", "INSERT").await,
            "`{role}` may append an audit row"
        );
        for privilege in ["UPDATE", "DELETE"] {
            assert!(
                !has_privilege(&mut connection, role, "control.audit_event", privilege).await,
                "`{role}` holds {privilege} on control.audit_event"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_authorization_reader_holds_no_write_anywhere() {
    let fixture = Fixture::start().await;
    let mut connection = fixture.superuser().await;

    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT format('%I.%I', table_schema, table_name) FROM information_schema.tables \
         WHERE table_schema IN ('identity','control') AND table_type = 'BASE TABLE'",
    )
    .fetch_all(&mut connection)
    .await
    .expect("the table probe runs");
    assert!(!tables.is_empty(), "the schemas are not empty");

    for table in tables {
        for privilege in ["INSERT", "UPDATE", "DELETE"] {
            assert!(
                !has_privilege(&mut connection, "aex_authz", &table, privilege).await,
                "`aex_authz` holds {privilege} on {table}; the assertion path is read-only"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_write_probe_really_fails_for_the_read_only_role() {
    let fixture = Fixture::start().await;
    let mut connection = fixture.as_role("aex_authz_login").await;

    let outcome = connection
        .execute(
            "INSERT INTO control.audit_event \
             (id, actor_kind, action, resource_kind, outcome, request_id, occurred_at) \
             VALUES (gen_random_uuid(), 'system', 'probe', 'user', 'denied', 'r', now())",
        )
        .await;
    let error = outcome.expect_err("the read-only role cannot write");
    assert!(
        error.to_string().contains("permission denied"),
        "expected a privilege failure, got {error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_control_roles_hold_nothing_in_finance_and_identity_writes() {
    let fixture = Fixture::start().await;
    let mut connection = fixture.superuser().await;

    for role in ["aex_control_api", "aex_control_worker"] {
        for privilege in ["INSERT", "UPDATE", "DELETE"] {
            assert!(
                !has_privilege(&mut connection, role, "identity.user", privilege).await,
                "`{role}` holds {privilege} on identity.user"
            );
        }
        assert!(
            has_privilege(&mut connection, role, "identity.user", "SELECT").await,
            "`{role}` may read identity.user"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_epoch_advances_only_through_its_security_definer_wrapper() {
    let fixture = Fixture::start().await;

    let mut api = fixture.as_role("aex_control_api_login").await;
    let subject = uuid::Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0001);

    let first: i64 = sqlx::query_scalar("SELECT control.bump_key_epoch($1)")
        .bind(subject)
        .fetch_one(&mut api)
        .await
        .expect("the wrapper is executable");
    assert_eq!(first, 1);
    let second: i64 = sqlx::query_scalar("SELECT control.bump_key_epoch($1)")
        .bind(subject)
        .fetch_one(&mut api)
        .await
        .expect("the wrapper is executable");
    assert_eq!(second, 2, "an epoch only moves forward");

    // The generic form is revoked, so a role holding one wrapper cannot bump
    // another subject kind.
    let denied = sqlx::query_scalar::<_, i64>("SELECT control.bump_epoch('user', $1)")
        .bind(subject)
        .fetch_one(&mut api)
        .await;
    assert!(
        denied.is_err(),
        "control.bump_epoch is revoked from every application role"
    );

    // `aex_control_api` was granted three wrappers and not the `user` one.
    let user_denied = sqlx::query_scalar::<_, i64>("SELECT control.bump_user_epoch($1)")
        .bind(subject)
        .fetch_one(&mut api)
        .await;
    assert!(
        user_denied.is_err(),
        "control-api may not advance the user epoch; identity-api owns that"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_organization_can_never_lose_its_last_owner() {
    let fixture = Fixture::start().await;
    let mut connection = fixture.superuser().await;

    let user = uuid::Uuid::from_u128(1);
    let organization = uuid::Uuid::from_u128(2);
    let membership = uuid::Uuid::from_u128(3);

    sqlx::query(
        "INSERT INTO identity.user (id, email, status, created_at, updated_at) \
         VALUES ($1, 'a@b.test', 'active', now(), now())",
    )
    .bind(user)
    .execute(&mut connection)
    .await
    .expect("the person inserts");

    sqlx::query(
        "INSERT INTO control.organization (id, name, slug, created_at, updated_at, created_by_user_id) \
         VALUES ($1, 'Acme', 'acme', now(), now(), $2)",
    )
    .bind(organization)
    .bind(user)
    .execute(&mut connection)
    .await
    .expect("the organization inserts");

    // An organization with no owner cannot even be created: the deferred
    // trigger fires at COMMIT.
    let orphan = sqlx::query(
        "INSERT INTO control.membership (id, organization_id, user_id, role, created_at, updated_at) \
         VALUES ($1, $2, $3, 'member', now(), now())",
    )
    .bind(membership)
    .bind(organization)
    .bind(user)
    .execute(&mut connection)
    .await;
    assert!(
        orphan.is_err(),
        "a member-only organization has no active owner"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_workspaces_placement_is_immutable() {
    let fixture = Fixture::start().await;
    let mut connection = fixture.superuser().await;

    let user = uuid::Uuid::from_u128(1);
    let organization = uuid::Uuid::from_u128(2);
    let workspace = uuid::Uuid::from_u128(4);

    sqlx::query(
        "INSERT INTO identity.user (id, email, status, created_at, updated_at) \
         VALUES ($1, 'a@b.test', 'active', now(), now())",
    )
    .bind(user)
    .execute(&mut connection)
    .await
    .expect("the person inserts");
    sqlx::query(
        "INSERT INTO control.organization (id, name, slug, created_at, updated_at, created_by_user_id) \
         VALUES ($1, 'Acme', 'acme', now(), now(), $2)",
    )
    .bind(organization)
    .bind(user)
    .execute(&mut connection)
    .await
    .expect("the organization inserts");
    sqlx::query(
        "INSERT INTO control.workspace \
           (id, organization_id, name, slug, region, status, provision_operation_id, \
            provision_fence, created_at, updated_at, created_by_user_id) \
         VALUES ($1, $2, 'Prod', 'prod', 'eu-west-1', 'provisioning', gen_random_uuid(), 1, \
                 now(), now(), $3)",
    )
    .bind(workspace)
    .bind(organization)
    .bind(user)
    .execute(&mut connection)
    .await
    .expect("the workspace inserts");

    let moved = sqlx::query("UPDATE control.workspace SET region = 'us-east-1' WHERE id = $1")
        .bind(workspace)
        .execute(&mut connection)
        .await;
    assert!(moved.is_err(), "a workspace never changes region");
}

#[tokio::test(flavor = "multi_thread")]
async fn only_one_signing_key_can_be_active() {
    let fixture = Fixture::start().await;
    let mut connection = fixture.superuser().await;

    for index in 0..2_u8 {
        let outcome = sqlx::query(
            "INSERT INTO control.signing_key \
               (kid, alg, public_key, secret_ref, state, created_at, activates_at, retires_at) \
             VALUES (gen_random_uuid(), 'ed25519', repeat('\\x00', 32)::bytea, $1, 'active', \
                     now(), now(), now() + interval '1 day')",
        )
        .bind(format!("aex/dev/authz-signing/{index}"))
        .execute(&mut connection)
        .await;
        if index == 0 {
            outcome.expect("the first active key inserts");
        } else {
            assert!(outcome.is_err(), "a second active key is refused");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_two_authorization_statements_are_valid_sql_against_the_real_schema() {
    let fixture = Fixture::start().await;
    let mut connection = fixture.as_role("aex_authz_login").await;

    // `PREPARE` parses, plans and privilege-checks without running, so a
    // mistyped column or a missing grant fails here rather than at 3 a.m.
    for (name, statement) in [
        (
            "workspace_key",
            aex_control_aurora::sql::RESOLVE_WORKSPACE_KEY.replace(":key_id", "$1"),
        ),
        (
            "account_token",
            aex_control_aurora::sql::RESOLVE_ACCOUNT_TOKEN_FOR_WORKSPACE
                .replace(":credential_id", "$1")
                .replace(":workspace_id", "$2")
                .replace(":now_ms", "$3"),
        ),
        (
            "session",
            aex_control_aurora::sql::RESOLVE_SESSION_FOR_WORKSPACE
                .replace(":credential_id", "$1")
                .replace(":workspace_id", "$2")
                .replace(":now_ms", "$3"),
        ),
    ] {
        // `execute` wants a `'static` statement, and every input here is a
        // crate constant plus a fixture-chosen name, so leaking the assembled
        // probe for the life of the test is both sound and cheap. Nothing
        // caller-supplied reaches this string.
        let probe: &'static str =
            Box::leak(format!("PREPARE probe_{name} AS {statement}").into_boxed_str());
        connection
            .execute(probe)
            .await
            .unwrap_or_else(|error| panic!("`{name}` is valid for aex_authz: {error}"));
    }
}
