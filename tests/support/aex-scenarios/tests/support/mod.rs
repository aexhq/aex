//! The central plane, migrated, granted and reachable through its own adapters.
//!
//! Every central scenario body starts here, and what it starts is deliberately
//! the production thing at each layer:
//!
//! - the schema is `central_schema_admin::migration::native_migrator()`, the
//!   same migrator a plane runs, not a shorter hand-written chain;
//! - the privileges are `central_schema_admin::grants::GrantSet::render()`, so
//!   a statement that production's role may not run fails here too;
//! - each store connects as the **login role its deployable connects as**, so a
//!   missing grant is a failing case rather than a fixture that quietly held
//!   superuser;
//! - the repository is the committed `Aurora*Store` over
//!   `aex_scenarios::PostgresDataApi`, so the statements, the binding, the row
//!   decoding, the transaction lifecycle and the `SQLSTATE` classification are
//!   all production code.
//!
//! What is *not* production here is named in each body: the engine is a
//! container rather than Aurora, and the regional half is a recording double
//! rather than a deployed region. Both are stated where they bite.
//!
//! # Failure, never skip
//!
//! A Docker daemon that refuses the container panics with the container error.
//! There is no "engine unavailable, therefore green" path anywhere in this file.

use std::sync::Arc;

use aex_rds_data::{DataApiClient, DataApiConfig, DatabaseName, ResourceArn, SecretArn};
use aex_scenarios::PostgresDataApi;
use aex_test_harness::containers::PostgresContainer;
use central_schema_admin::grants::GrantSet;
use central_schema_admin::migration::native_migrator;
use sqlx::{Connection as _, Executor as _, PgConnection};
use tokio::sync::OnceCell;

/// The login roles `central-schema-admin` attaches in production.
///
/// Copied from `crates/aex-control-aurora/tests/migrations.rs` rather than
/// abbreviated: a scenario that connected as one role while production connects
/// as another would prove the wrong denial matrix.
const LOGIN_ROLES: &str = "\
CREATE ROLE aex_identity_api_login   LOGIN PASSWORD 'fixture'; \
CREATE ROLE aex_authz_login          LOGIN PASSWORD 'fixture'; \
CREATE ROLE aex_control_api_login    LOGIN PASSWORD 'fixture'; \
CREATE ROLE aex_control_worker_login LOGIN PASSWORD 'fixture'; \
CREATE ROLE aex_finance_api_login    LOGIN PASSWORD 'fixture'; \
CREATE ROLE aex_finance_ingest_login LOGIN PASSWORD 'fixture'; \
GRANT aex_identity_api   TO aex_identity_api_login; \
GRANT aex_authz          TO aex_authz_login; \
GRANT aex_control_api    TO aex_control_api_login; \
GRANT aex_control_worker TO aex_control_worker_login; \
GRANT aex_finance_api    TO aex_finance_api_login; \
GRANT aex_finance_ingest TO aex_finance_ingest_login;";

/// The one engine a test process runs against.
static ENGINE: OnceCell<Arc<PostgresContainer>> = OnceCell::const_new();

/// The committed privilege model, parsed once.
fn grants() -> &'static GrantSet {
    static GRANTS: std::sync::OnceLock<GrantSet> = std::sync::OnceLock::new();
    GRANTS.get_or_init(|| {
        GrantSet::embedded().unwrap_or_else(|error| panic!("the committed grants parse: {error}"))
    })
}

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

/// One migrated central database, with the production privileges applied.
pub struct CentralPlane {
    admin_url: String,
    database: String,
    _engine: Arc<PostgresContainer>,
}

impl CentralPlane {
    /// Mints and migrates a database of this case's own.
    pub async fn start() -> Self {
        let engine = engine().await;
        let admin_url = engine.connection_url();
        let database = format!("aex_{}", uuid::Uuid::new_v4().simple());
        let mut admin = PgConnection::connect(&admin_url)
            .await
            .unwrap_or_else(|error| panic!("the pinned engine is reachable: {error}"));
        let create: &'static str =
            Box::leak(format!("CREATE DATABASE {database}").into_boxed_str());
        admin
            .execute(create)
            .await
            .expect("the case database is created");
        let plane = Self {
            admin_url,
            database,
            _engine: engine,
        };
        plane.migrate().await;
        plane
    }

    /// Applies the committed bundle, the rendered privileges and the login roles.
    async fn migrate(&self) {
        let mut connection = self.superuser().await;
        native_migrator()
            .run(&mut connection)
            .await
            .unwrap_or_else(|error| panic!("the committed migration bundle applies: {error}"));
        for statement in grants()
            .render(&self.database)
            .expect("the privilege model renders")
        {
            let statement: &'static str = Box::leak(statement.into_boxed_str());
            connection
                .execute(statement)
                .await
                .unwrap_or_else(|error| panic!("`{statement}` applies: {error}"));
        }
        if let Err(error) = connection.execute(LOGIN_ROLES).await {
            // The roles are cluster-wide, so a parallel case may have created
            // them already. Their existence is the point, not their author.
            assert!(
                error.to_string().contains("already exists"),
                "the login roles attach: {error}"
            );
        }
    }

    /// The `host:port` this run reached, without credentials or database.
    fn authority(&self) -> String {
        let without_database = self
            .admin_url
            .rsplit_once('/')
            .map_or_else(|| self.admin_url.clone(), |(head, _)| head.to_owned());
        without_database
            .rsplit_once('@')
            .map_or(without_database.clone(), |(_, host)| host.to_owned())
    }

    /// The superuser URL for this case's database.
    fn superuser_url(&self) -> String {
        let without_database = self
            .admin_url
            .rsplit_once('/')
            .map_or_else(|| self.admin_url.clone(), |(head, _)| head.to_owned());
        format!("{without_database}/{}", self.database)
    }

    /// The URL for one production login role.
    fn role_url(&self, role: &str) -> String {
        let scheme = self
            .admin_url
            .split_once("://")
            .map_or("postgres", |(scheme, _)| scheme);
        format!(
            "{scheme}://{role}_login:fixture@{}/{}",
            self.authority(),
            self.database
        )
    }

    /// A superuser connection, for seeding rows no production writer owns yet
    /// and for reading a table the role under test may not read.
    pub async fn superuser(&self) -> PgConnection {
        PgConnection::connect(&self.superuser_url())
            .await
            .unwrap_or_else(|error| panic!("the case database is reachable: {error}"))
    }

    /// A Data `API` client connected as one production login role.
    pub async fn client_as(&self, role: &str) -> DataApiClient {
        let transport = PostgresDataApi::connect(&self.role_url(role))
            .await
            .unwrap_or_else(|error| panic!("role `{role}` connects: {error}"));
        DataApiClient::new(Arc::new(transport), config())
    }
}

/// The transport configuration.
///
/// The ARNs are inert here — the transport reaches a container, not AWS — but
/// the byte budgets are the production defaults, so a result this platform would
/// refuse in production is refused here too.
fn config() -> DataApiConfig {
    DataApiConfig::new(
        ResourceArn::parse("arn:aws:rds:eu-west-1:000000000000:cluster:aex").expect("cluster arn"),
        SecretArn::parse("arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex-x")
            .expect("secret arn"),
        DatabaseName::parse("aex").expect("database name"),
    )
}
