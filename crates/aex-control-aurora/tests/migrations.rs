//! The central migration bundle against real `PostgreSQL`.
//!
//! What this proves that no unit test can: that the DDL applies at all, that
//! every `CHECK`, unique index, trigger and `SECURITY DEFINER` function behaves
//! as written, and — most importantly — that the **role-denial matrix** is real.
//! A grant table nobody connects as is a document, not a control.
//!
//! The fixture applies the whole committed chain and then
//! `central_schema_admin::grants::GrantSet::render`, which is the same renderer
//! `central-schema-admin grants --apply` runs against a plane. Nothing here
//! grants a privilege of its own — not `CONNECT`, not `SELECT`, not `EXECUTE` —
//! so a privilege these cases rely on and production lacks is a failing case
//! rather than a fixture that quietly propped the suite up.
//!
//! What it cannot prove is that Aurora's Data `API` behaves like a direct
//! connection; the `aws.rds_data.transaction` seam is claimed by the live
//! companion.
//!
//! # The engine
//!
//! One digest-pinned `PostgreSQL` container per process, started through
//! `aex_test_harness::containers::PostgresContainer`. The suite writes no image
//! name, tag or digest of its own — the `data-image-literal` rule bans that
//! everywhere outside the harness, and the harness is the one place a pin is
//! reviewed.
//!
//! Every case mints its **own** database inside that server, so the cases
//! parallelise without sharing a schema while paying for one container. A
//! Docker daemon that refuses the container is a failure, never a skip: the
//! whole suite panics with the container error rather than reporting green.

use std::sync::Arc;

use aex_test_harness::containers::PostgresContainer;
use central_schema_admin::grants::GrantSet;
use sqlx::{Connection, Executor as _, PgConnection, Row as _};
use tokio::sync::OnceCell;

/// The whole committed chain, identity through finance, in order.
///
/// There is no per-stream subset and no stub for the peer's objects: the
/// statements this crate owns join `finance.account_state_v1` and call
/// `finance.ensure_account`, and a fixture-shaped stand-in would prove only that
/// the fixture agrees with itself.
const MIGRATIONS: [(&str, &str); 8] = [
    (
        "20260801000000_bootstrap",
        include_str!("../../../migrations/central/20260801000000_bootstrap.sql"),
    ),
    (
        "20260801000100_identity",
        include_str!("../../../migrations/central/20260801000100_identity.sql"),
    ),
    (
        "20260801000200_control",
        include_str!("../../../migrations/central/20260801000200_control.sql"),
    ),
    (
        "20260801000300_control_functions",
        include_str!("../../../migrations/central/20260801000300_control_functions.sql"),
    ),
    (
        "20260801000400_finance_roles_and_schema",
        include_str!("../../../migrations/central/20260801000400_finance_roles_and_schema.sql"),
    ),
    (
        "20260801000500_baseline_finance",
        include_str!("../../../migrations/central/20260801000500_baseline_finance.sql"),
    ),
    (
        "20260801000600_baseline_seed_platform_accounts",
        include_str!(
            "../../../migrations/central/20260801000600_baseline_seed_platform_accounts.sql"
        ),
    ),
    (
        "20260801000700_finance_account_state",
        include_str!("../../../migrations/central/20260801000700_finance_account_state.sql"),
    ),
];

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

/// The committed privilege model, parsed once.
fn grants() -> &'static GrantSet {
    static GRANTS: std::sync::OnceLock<GrantSet> = std::sync::OnceLock::new();
    GRANTS.get_or_init(|| {
        GrantSet::load(central_schema_admin::grants::grants_path())
            .unwrap_or_else(|error| panic!("the committed grants.toml parses: {error}"))
    })
}

/// The one engine this process runs against.
///
/// Shared because a container per case would pay a start-up cost sixteen times
/// for a server every case immediately isolates itself inside anyway.
static ENGINE: OnceCell<Arc<PostgresContainer>> = OnceCell::const_new();

/// The running engine, started on first use.
async fn engine() -> Arc<PostgresContainer> {
    ENGINE
        .get_or_init(|| async {
            Arc::new(
                PostgresContainer::start()
                    .await
                    .unwrap_or_else(|error| panic!("the pinned PostgreSQL starts: {error}")),
            )
        })
        .await
        .clone()
}

/// A freshly migrated database.
///
/// Each case mints its own database inside the supplied server, so the suite
/// parallelises without two cases sharing a schema.
struct Fixture {
    admin_url: String,
    database: String,
    /// Keeps the shared engine alive for as long as any fixture uses it.
    _engine: Arc<PostgresContainer>,
}

impl Fixture {
    async fn start() -> Self {
        let engine = engine().await;
        let admin_url = engine.connection_url();
        let database = format!("aex_{}", uuid::Uuid::new_v4().simple());
        let mut admin = PgConnection::connect(&admin_url)
            .await
            .unwrap_or_else(|error| panic!("the pinned engine is reachable: {error}"));
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
            _engine: engine,
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

    /// Applies the bundle, then the declared privileges, then the login roles.
    async fn migrate(&self) {
        let mut connection = self.superuser().await;
        for (name, sql) in MIGRATIONS {
            // Migration bodies carry no privilege and no database name, so they
            // reach the fixture database exactly as they reach a plane.
            connection
                .execute(sql)
                .await
                .unwrap_or_else(|error| panic!("`{name}` applies: {error}"));
        }
        // The production privilege model, rendered from the committed document
        // by the production renderer and pointed at this case's database. Every
        // `CONNECT`, `USAGE`, table privilege and `EXECUTE` the cases below
        // exercise arrives through this and nowhere else, so it also proves that
        // every object `grants.toml` names actually exists — PostgreSQL refuses
        // a grant on a table or function that does not.
        for statement in grants().render(&self.database).expect("the model renders") {
            let statement: &'static str = Box::leak(statement.into_boxed_str());
            connection
                .execute(statement)
                .await
                .unwrap_or_else(|error| panic!("`{statement}` applies: {error}"));
        }
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

    for schema in ["identity", "control", "finance", "schema_admin"] {
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
async fn no_role_can_open_a_session_without_its_declared_connect() {
    let fixture = Fixture::start().await;
    let mut connection = fixture.superuser().await;

    // `PUBLIC` lost every database privilege, so `CONNECT` reaches a role only
    // through `grants.toml`. Both halves matter: a role that cannot connect is
    // an outage, and a `PUBLIC` that can is the revoke undone.
    let public: bool = sqlx::query_scalar("SELECT has_database_privilege('public', $1, 'CONNECT')")
        .bind(&fixture.database)
        .fetch_one(&mut connection)
        .await
        .expect("the database privilege probe runs");
    assert!(!public, "PUBLIC may not open a session on the database");

    for role in grants().role_names() {
        let connects: bool = sqlx::query_scalar("SELECT has_database_privilege($1, $2, 'CONNECT')")
            .bind(role)
            .bind(&fixture.database)
            .fetch_one(&mut connection)
            .await
            .expect("the database privilege probe runs");
        assert!(connects, "`{role}` cannot open a session at all");
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
         WHERE table_schema IN ('identity','control','finance') AND table_type = 'BASE TABLE'",
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

    // The whole finance surface either control role may reach is one view, and
    // even that is read-only. Everything else — the ledger, the balances, the
    // provider edges — is out of reach in every mode, including `SELECT`.
    let finance: Vec<String> = sqlx::query_scalar(
        "SELECT format('%I.%I', table_schema, table_name) FROM information_schema.tables \
         WHERE table_schema = 'finance'",
    )
    .fetch_all(&mut connection)
    .await
    .expect("the table probe runs");
    assert!(
        finance.contains(&"finance.account_state_v1".to_owned()),
        "the published projection exists as a real object, not a fixture stub"
    );

    for role in ["aex_control_api", "aex_control_worker"] {
        for object in &finance {
            for privilege in ["SELECT", "INSERT", "UPDATE", "DELETE"] {
                let expected = role == "aex_control_api"
                    && object == "finance.account_state_v1"
                    && privilege == "SELECT";
                assert_eq!(
                    has_privilege(&mut connection, role, object, privilege).await,
                    expected,
                    "`{role}` privilege {privilege} on {object} is wrong"
                );
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn establishing_an_account_is_idempotent_and_needs_no_finance_write() {
    let fixture = Fixture::start().await;
    let mut superuser = fixture.superuser().await;

    let user = uuid::Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0010);
    let organization = uuid::Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0011);
    sqlx::query(
        "INSERT INTO identity.user (id, email, status, created_at, updated_at) \
         VALUES ($1, 'founder@b.test', 'active', now(), now())",
    )
    .bind(user)
    .execute(&mut superuser)
    .await
    .expect("the person inserts");

    let mut api = fixture.as_role("aex_control_api_login").await;
    sqlx::query(
        "INSERT INTO control.organization (id, name, slug, created_at, updated_at, created_by_user_id) \
         VALUES ($1, 'Acme', 'acme', now(), now(), $2)",
    )
    .bind(organization)
    .bind(user)
    .execute(&mut api)
    .await
    .expect("control-api inserts the organization");

    // `SECURITY DEFINER` is the whole point: the role holds no INSERT anywhere
    // in `finance` and still establishes the account.
    assert!(
        !has_privilege(
            &mut superuser,
            "aex_control_api",
            "finance.billing_account",
            "INSERT"
        )
        .await,
        "control-api must reach the billing account only through the wrapper"
    );

    let created: bool = sqlx::query_scalar("SELECT finance.ensure_account($1)")
        .bind(organization)
        .fetch_one(&mut api)
        .await
        .expect("the wrapper is executable by control-api");
    assert!(created, "the first call establishes the account");

    let again: bool = sqlx::query_scalar("SELECT finance.ensure_account($1)")
        .bind(organization)
        .fetch_one(&mut api)
        .await
        .expect("the wrapper is idempotent");
    assert!(!again, "a replay converges and reports it created nothing");

    // Two customer accounts, each with a balance row, exactly once.
    let accounts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM finance.account a \
           JOIN finance.account_balance b ON b.account_id = a.account_id \
          WHERE a.org_id = $1",
    )
    .bind(organization)
    .fetch_one(&mut superuser)
    .await
    .expect("the account probe runs");
    assert_eq!(accounts, 2, "available and reserved, and no duplicates");

    let state: String = sqlx::query_scalar(
        "SELECT status FROM finance.account_state_v1 WHERE organization_id = $1",
    )
    .bind(organization)
    .fetch_one(&mut superuser)
    .await
    .expect("the projection reads");
    assert_eq!(state, "active");
}

#[tokio::test(flavor = "multi_thread")]
async fn every_held_account_reads_as_paused_and_never_as_active() {
    let fixture = Fixture::start().await;
    let mut connection = fixture.superuser().await;

    let user = uuid::Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0020);
    sqlx::query(
        "INSERT INTO identity.user (id, email, status, created_at, updated_at) \
         VALUES ($1, 'held@b.test', 'active', now(), now())",
    )
    .bind(user)
    .execute(&mut connection)
    .await
    .expect("the person inserts");

    // Every state the billing account admits other than `active`. The view maps
    // all of them, and anything added later, to the restrictive answer.
    for (index, held) in ["payment_hold", "dispute_hold", "closed"]
        .into_iter()
        .enumerate()
    {
        let organization =
            uuid::Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0030 + index as u128);
        sqlx::query(
            "INSERT INTO control.organization (id, name, slug, created_at, updated_at, created_by_user_id) \
             VALUES ($1, 'Held', $2, now(), now(), $3)",
        )
        .bind(organization)
        .bind(held.replace('_', "-"))
        .bind(user)
        .execute(&mut connection)
        .await
        .expect("the organization inserts");
        sqlx::query("SELECT finance.ensure_account($1)")
            .bind(organization)
            .execute(&mut connection)
            .await
            .expect("the account is established");
        sqlx::query(
            "UPDATE finance.billing_account \
                SET state = $2, state_reason = 'fixture', updated_at = now() WHERE org_id = $1",
        )
        .bind(organization)
        .bind(held)
        .execute(&mut connection)
        .await
        .expect("the hold applies");

        let read: &'static str = Box::leak(
            aex_control_aurora::sql::GET_ACCOUNT_STATE
                .replace(":organization_id", "$1")
                .into_boxed_str(),
        );
        let status: String = sqlx::query_scalar(read)
            .bind(organization)
            .fetch_one(&mut connection)
            .await
            .expect("the account-state read runs");
        assert_eq!(
            status, "paused_top_up_required",
            "`{held}` must not read as active"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_account_state_change_advances_its_epoch_and_enqueues_every_live_workspace() {
    let fixture = Fixture::start().await;
    let mut connection = fixture.superuser().await;
    let user = uuid::Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0040);
    let organization = uuid::Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0041);
    let workspace = uuid::Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0042);
    let operation = uuid::Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0043);
    sqlx::query(
        "INSERT INTO identity.user (id, email, status, created_at, updated_at) \
         VALUES ($1, 'account-feed@b.test', 'active', now(), now())",
    )
    .bind(user)
    .execute(&mut connection)
    .await
    .expect("person inserts");
    sqlx::query(
        "INSERT INTO control.organization \
           (id, name, slug, created_at, updated_at, created_by_user_id) \
         VALUES ($1, 'Account Feed', 'account-feed', now(), now(), $2)",
    )
    .bind(organization)
    .bind(user)
    .execute(&mut connection)
    .await
    .expect("organization inserts");
    sqlx::query("SELECT finance.ensure_account($1)")
        .bind(organization)
        .execute(&mut connection)
        .await
        .expect("account inserts and initializes its epoch");
    sqlx::query(
        "INSERT INTO control.workspace \
           (id, organization_id, name, slug, region, status, provision_operation_id, \
            provision_fence, created_at, updated_at, activated_at, created_by_user_id) \
         VALUES ($1, $2, 'Production', 'production', 'eu-west-1', 'active', $3, 1, \
                 now(), now(), now(), $4)",
    )
    .bind(workspace)
    .bind(organization)
    .bind(operation)
    .bind(user)
    .execute(&mut connection)
    .await
    .expect("workspace inserts");

    sqlx::query(
        "UPDATE finance.billing_account \
            SET state = 'payment_hold', state_reason = 'top_up_required' \
          WHERE org_id = $1",
    )
    .bind(organization)
    .execute(&mut connection)
    .await
    .expect("pause commits");

    let epoch: i64 = sqlx::query_scalar(
        "SELECT epoch FROM control.authorization_epoch \
          WHERE subject_kind = 'account' AND subject_id = $1",
    )
    .bind(organization)
    .fetch_one(&mut connection)
    .await
    .expect("account epoch exists");
    assert_eq!(epoch, 2, "insert initialized one and pause advanced it");
    let payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM control.outbox_message \
          WHERE topic = 'account.state.changed' AND payload->>'workspaceId' = $1",
    )
    .bind(workspace.to_string())
    .fetch_one(&mut connection)
    .await
    .expect("the workspace projection message exists");
    assert_eq!(payload["organizationId"], organization.to_string());
    assert_eq!(payload["accountEpoch"], 2);
    assert_eq!(payload["accountRevision"], 1);
    assert!(payload["changedAtMs"].as_i64().is_some());
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
        // `decode(repeat('00',32),'hex')` and not `repeat('\x00',32)::bytea`:
        // the second builds the 128-character *text* `\x00\x00…` and asks the
        // hex input parser to read it, which fails on the first backslash. The
        // insert then never happened, and the case reported a `CHECK` it had
        // not reached.
        let outcome = sqlx::query(
            "INSERT INTO control.signing_key \
               (kid, alg, public_key, secret_ref, state, created_at, activates_at, retires_at) \
             VALUES (gen_random_uuid(), 'ed25519', decode(repeat('00', 32), 'hex'), $1, 'active', \
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
        // Counting is what stops this case regressing to what it used to be.
        // With a `public_key` the hex parser refused, the first insert never
        // happened either, and "the second is refused" was then true for the
        // wrong reason — the uniqueness rule was never exercised at all.
        let active: i64 =
            sqlx::query_scalar("SELECT count(*) FROM control.signing_key WHERE state = 'active'")
                .fetch_one(&mut connection)
                .await
                .expect("the key probe runs");
        assert_eq!(
            active, 1,
            "exactly one active key exists after attempt {index}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn every_authorization_statement_is_valid_sql_against_the_real_schema() {
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
        (
            "central_account_token",
            aex_control_aurora::sql::RESOLVE_ACCOUNT_TOKEN_CENTRAL
                .replace(":credential_id", "$1")
                .replace(":now_ms", "$2"),
        ),
        (
            "central_session",
            aex_control_aurora::sql::RESOLVE_SESSION_CENTRAL
                .replace(":credential_id", "$1")
                .replace(":now_ms", "$2"),
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

#[tokio::test(flavor = "multi_thread")]
async fn the_coarse_account_state_read_is_valid_and_permitted_for_the_control_api_role() {
    let fixture = Fixture::start().await;
    let mut connection = fixture.as_role("aex_control_api_login").await;

    let statement = aex_control_aurora::sql::GET_ACCOUNT_STATE.replace(":organization_id", "$1");
    let probe: &'static str =
        Box::leak(format!("PREPARE probe_account_state AS {statement}").into_boxed_str());
    connection.execute(probe).await.unwrap_or_else(|error| {
        panic!("the account-state read is valid for aex_control_api: {error}")
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn an_organization_with_no_finance_row_reads_as_unavailable_and_never_as_active() {
    let fixture = Fixture::start().await;
    let mut connection = fixture.superuser().await;

    // `query_scalar` wants a `'static` statement, and the input is a crate
    // constant with one parameter marker rewritten; nothing caller-supplied
    // reaches this string.
    let read: &'static str = Box::leak(
        aex_control_aurora::sql::GET_ACCOUNT_STATE
            .replace(":organization_id", "$1")
            .into_boxed_str(),
    );

    // An organization nobody ever created. From the edge's point of view this is
    // the same question as the one below: nobody established the state.
    let absent: Vec<String> = sqlx::query_scalar(read)
        .bind(uuid::Uuid::from_u128(0xDEAD))
        .fetch_all(&mut connection)
        .await
        .expect("the account-state read runs");
    assert!(
        absent.is_empty(),
        "an unknown organization must not answer a status at all, got {absent:?}"
    );

    // An organization that exists and whose `finance.ensure_account` never ran.
    // This is the case the fixture stub could not model at all — its view was
    // derived from `control.organization`, so every organization it knew about
    // read `active`, which is precisely the wrong default.
    let user = uuid::Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0040);
    let organization = uuid::Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0041);
    sqlx::query(
        "INSERT INTO identity.user (id, email, status, created_at, updated_at) \
         VALUES ($1, 'nofinance@b.test', 'active', now(), now())",
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

    let unestablished: String = sqlx::query_scalar(read)
        .bind(organization)
        .fetch_one(&mut connection)
        .await
        .expect("the account-state read runs");
    assert_eq!(
        unestablished, "unavailable",
        "a missing finance row is never active; it is `503 account_state_unavailable`"
    );
}
