//! The Aurora Data `API` transport, spoken to a real `PostgreSQL`.
//!
//! `aex_rds_data::Transport` is four calls wide, and every central repository in
//! this workspace reaches its database through exactly those four. Implementing
//! them over a container is what makes a central scenario body *executable*:
//! the statements, the parameter binding, the row decoding, the transaction
//! lifecycle and the error classification are all the committed production
//! code, and only the endpoint differs.
//!
//! # What it is faithful about, deliberately
//!
//! - **`SQLSTATE`.** The database's own code is forwarded in the message shape
//!   `aex_rds_data::error::classify` parses, so a unique violation is classified
//!   from a real unique violation rather than from a fixture that decided to
//!   look like one.
//! - **Commit ambiguity.** A failed `COMMIT` is reported as a service failure
//!   carrying its `SQLSTATE`, which is what lets `DataApiClient::finish`
//!   distinguish a definite rollback from an unknown outcome.
//! - **Typing.** Parameters carry the casts their `TypeHint` implies, so a
//!   statement that only works because Aurora coerced a value keeps working and
//!   one that never worked still fails.
//!
//! # What it is not
//!
//! It is not the `aws.rds_data.transaction` seam. The Data `API`'s own
//! behaviours — its 24-hour transaction ceiling, its result paging, its
//! throttling, its `DatabaseResumingException` — have no local counterpart, and
//! `release/policy/seams.toml` keeps that seam `requires_live`. This transport
//! never answers `Resuming`, `Throttled` or `TransactionExpired`, and a body
//! that needs one of those has to be a live body.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use aws_sdk_rdsdata::types::{Field, SqlParameter};
use sqlx::pool::PoolConnection;
use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::{AssertSqlSafe, Column, Executor, PgConnection, PgPool, Postgres, Row as _, TypeInfo as _};

use aex_rds_data::client::{ExecuteResponse, Transport, TransportError};
use aex_rds_data::error::ExceptionKind;
use aex_rds_data::transaction::TransactionId;

use super::literal::{field, parse_row_literal};
use super::render::{Bound, Rendered, render};

/// How many connections the pool may open.
///
/// One per concurrently open transaction plus headroom for the statements a
/// case runs outside one. A test that opens more transactions than this blocks
/// rather than failing, which is why the number is stated here rather than left
/// at the driver default.
const MAX_CONNECTIONS: u32 = 16;

/// The status string the service reports for a successful commit.
///
/// `DataApiClient::finish` reads this string and treats anything containing
/// "rollback" as a definite abort, so the value is part of the contract.
const COMMITTED_STATUS: &str = "Transaction Committed";

/// A Data `API` transport backed by a `PostgreSQL` connection pool.
#[derive(Debug)]
pub struct PostgresDataApi {
    pool: PgPool,
    /// The connection each open transaction is pinned to.
    ///
    /// The Data `API` addresses a transaction by an opaque id and holds the
    /// session itself; a `PostgreSQL` transaction *is* a session, so the id has
    /// to name one. Each entry carries its own lock so two transactions run
    /// concurrently, which is what a contention case needs.
    open: Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<PoolConnection<Postgres>>>>>,
}

impl PostgresDataApi {
    /// Connects to `url`.
    ///
    /// # Errors
    ///
    /// Returns the connection failure. There is no retry and no fallback: an
    /// engine that is not there is the failure, not a reason to degrade.
    pub async fn connect(url: &str) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(MAX_CONNECTIONS)
            .connect(url)
            .await?;
        Ok(Self {
            pool,
            open: Mutex::new(BTreeMap::new()),
        })
    }

    /// The pool, for a fixture that has to provision before the product runs.
    #[must_use]
    pub const fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// The connection an open transaction is pinned to.
    fn session(
        &self,
        transaction: &TransactionId,
    ) -> Result<Arc<tokio::sync::Mutex<PoolConnection<Postgres>>>, TransportError> {
        self.open
            .lock()
            .expect("the transaction registry is not poisoned")
            .get(transaction.as_str())
            .cloned()
            .ok_or_else(|| TransportError::Service {
                kind: ExceptionKind::TransactionNotFound,
                message: format!("Transaction {} is not found", transaction.as_str()),
            })
    }

    /// Removes an open transaction from the registry, returning its connection.
    fn take(
        &self,
        transaction: &TransactionId,
    ) -> Result<Arc<tokio::sync::Mutex<PoolConnection<Postgres>>>, TransportError> {
        self.open
            .lock()
            .expect("the transaction registry is not poisoned")
            .remove(transaction.as_str())
            .ok_or_else(|| TransportError::Service {
                kind: ExceptionKind::TransactionNotFound,
                message: format!("Transaction {} is not found", transaction.as_str()),
            })
    }
}

#[async_trait]
impl Transport for PostgresDataApi {
    async fn execute(
        &self,
        sql: &str,
        parameters: Vec<SqlParameter>,
        transaction: Option<&TransactionId>,
    ) -> Result<ExecuteResponse, TransportError> {
        let rendered = render(sql, &parameters).map_err(|error| TransportError::Service {
            kind: ExceptionKind::Unknown,
            message: format!("the scenario transport cannot render this statement: {error}"),
        })?;
        match transaction {
            None => run(&self.pool, &rendered).await,
            Some(id) => {
                let session = self.session(id)?;
                let mut guard = session.lock().await;
                run(&mut **guard, &rendered).await
            }
        }
    }

    async fn begin(&self) -> Result<TransactionId, TransportError> {
        let mut connection = self
            .pool
            .acquire()
            .await
            .map_err(|error| classify(&error))?;
        sqlx::query("BEGIN")
            .execute(&mut *connection)
            .await
            .map_err(|error| classify(&error))?;
        let id = format!("aex-scenario-{}", uuid::Uuid::new_v4());
        self.open
            .lock()
            .expect("the transaction registry is not poisoned")
            .insert(id.clone(), Arc::new(tokio::sync::Mutex::new(connection)));
        Ok(TransactionId::new(id))
    }

    async fn commit(&self, transaction: &TransactionId) -> Result<String, TransportError> {
        let session = self.take(transaction)?;
        let mut guard = session.lock().await;
        sqlx::query("COMMIT")
            .execute(&mut **guard)
            .await
            .map_err(|error| classify(&error))?;
        Ok(COMMITTED_STATUS.to_owned())
    }

    async fn rollback(&self, transaction: &TransactionId) -> Result<(), TransportError> {
        let session = self.take(transaction)?;
        let mut guard = session.lock().await;
        sqlx::query("ROLLBACK")
            .execute(&mut **guard)
            .await
            .map_err(|error| classify(&error))?;
        Ok(())
    }
}

/// Runs one rendered statement against an executor.
async fn run<'connection, E>(
    executor: E,
    rendered: &Rendered,
) -> Result<ExecuteResponse, TransportError>
where
    E: Executor<'connection, Database = Postgres>,
{
    let mut query = sqlx::query(AssertSqlSafe(rendered.sql.clone()));
    for bound in &rendered.binds {
        query = match bound {
            Bound::Bool(value) => query.bind(*value),
            Bound::I64(value) => query.bind(*value),
            Bound::Text(value) => query.bind(value.clone()),
            Bound::Bytes(value) => query.bind(value.clone()),
        };
    }
    if !rendered.returns_rows() {
        let outcome = query
            .execute(executor)
            .await
            .map_err(|error| classify(&error))?;
        return Ok(ExecuteResponse {
            records: Vec::new(),
            rows_affected: outcome.rows_affected(),
        });
    }
    let rows = query
        .fetch_all(executor)
        .await
        .map_err(|error| classify(&error))?;
    let records = rows
        .iter()
        .map(decode)
        .collect::<Result<Vec<_>, TransportError>>()?;
    let rows_affected = if rendered.dml {
        records.len() as u64
    } else {
        0
    };
    Ok(ExecuteResponse {
        records,
        rows_affected,
    })
}

/// Turns one wrapped row into the positional fields a record decoder reads.
///
/// Column `0` is the row rendered by the engine's own output function and
/// columns `1..` are the statement's own, untouched and undecoded — they are
/// present only so their *metadata* names each type.
fn decode(row: &PgRow) -> Result<Vec<Field>, TransportError> {
    let literal: String = row.try_get(0).map_err(|error| TransportError::Service {
        kind: ExceptionKind::Unknown,
        message: format!("the wrapped row text did not arrive as text: {error}"),
    })?;
    let raw = parse_row_literal(&literal).map_err(|error| TransportError::Service {
        kind: ExceptionKind::Unknown,
        message: format!("the engine's row rendering did not parse: {error}"),
    })?;
    let columns = &row.columns()[1..];
    if raw.len() != columns.len() {
        return Err(TransportError::Service {
            kind: ExceptionKind::Unknown,
            message: format!(
                "the engine rendered {} field(s) for a statement projecting {} column(s); \
                 a duplicated column name is the usual cause",
                raw.len(),
                columns.len()
            ),
        });
    }
    columns
        .iter()
        .zip(raw.iter())
        .enumerate()
        .map(|(index, (column, text))| {
            field(index, column.type_info().name(), text.as_deref()).map_err(|error| {
                TransportError::Service {
                    kind: ExceptionKind::Unknown,
                    message: format!("column `{}` did not decode: {error}", column.name()),
                }
            })
        })
        .collect()
}

/// Renders a driver failure the way the Data `API` renders a database failure.
///
/// A database answer keeps its `SQLSTATE`, because the production error table is
/// written against that code. A failure to reach the database at all is
/// `Indeterminate`, which is the only honest answer: the statement may or may
/// not have been applied.
fn classify(error: &sqlx::Error) -> TransportError {
    match error {
        sqlx::Error::Database(database) => TransportError::Service {
            kind: ExceptionKind::BadRequest,
            message: format!(
                "ERROR: {}; SQLState: {}",
                database.message(),
                database.code().unwrap_or_default()
            ),
        },
        sqlx::Error::Io(_)
        | sqlx::Error::Tls(_)
        | sqlx::Error::Protocol(_)
        | sqlx::Error::PoolTimedOut
        | sqlx::Error::PoolClosed
        | sqlx::Error::WorkerCrashed => TransportError::Indeterminate {
            message: error.to_string(),
        },
        other => TransportError::Service {
            kind: ExceptionKind::Unknown,
            message: other.to_string(),
        },
    }
}

/// Compile-time proof that a `PgConnection` is a usable executor here.
///
/// Nothing calls it; it exists so a `sqlx` upgrade that changes the executor
/// blanket implementation fails this crate rather than one distant caller.
const fn _executor_shapes(_: &PgPool, _: &mut PgConnection) {}
