//! SQL discipline, and the pinned per-assertion I/O budget.
//!
//! The budget is counted against a stub transport rather than asserted in a
//! comment: an assertion issue is exactly one statement and zero transactions,
//! and a change that adds a round trip fails here rather than in a latency
//! graph.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use aws_sdk_rdsdata::types::{ArrayValue, Field, SqlParameter};
use aws_smithy_types::Blob;
use time::OffsetDateTime;
use uuid::Uuid;

use aex_control_app::ports::AuthorizationReader;
use aex_control_aurora::{AuroraAuthorizationReader, sql};
use aex_rds_data::client::ExecuteResponse;
use aex_rds_data::{
    DataApiClient, DataApiConfig, DatabaseName, ResourceArn, SecretArn, TransactionId, Transport,
    TransportError,
};

/// What the stub was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Execute {
        sql: String,
        parameters: Vec<String>,
    },
    Begin,
    Commit,
    Rollback,
}

#[derive(Debug, Default)]
struct Counting {
    calls: Mutex<Vec<Call>>,
    records: Mutex<Vec<Vec<Field>>>,
}

impl Counting {
    fn with(records: Vec<Vec<Field>>) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            records: Mutex::new(records),
        })
    }

    fn calls(self: &Arc<Self>) -> Vec<Call> {
        self.calls.lock().expect("calls").clone()
    }

    fn statements(self: &Arc<Self>) -> usize {
        self.calls()
            .iter()
            .filter(|call| matches!(call, Call::Execute { .. }))
            .count()
    }

    fn transactions(self: &Arc<Self>) -> usize {
        self.calls()
            .iter()
            .filter(|call| matches!(call, Call::Begin | Call::Commit | Call::Rollback))
            .count()
    }
}

#[async_trait]
impl Transport for Counting {
    async fn execute(
        &self,
        sql: &str,
        parameters: Vec<SqlParameter>,
        _transaction: Option<&TransactionId>,
    ) -> Result<ExecuteResponse, TransportError> {
        self.calls.lock().expect("calls").push(Call::Execute {
            sql: sql.to_owned(),
            parameters: parameters
                .iter()
                .map(|it| it.name().unwrap_or_default().to_owned())
                .collect(),
        });
        Ok(ExecuteResponse {
            records: std::mem::take(&mut *self.records.lock().expect("records")),
            rows_affected: 0,
        })
    }

    async fn begin(&self) -> Result<TransactionId, TransportError> {
        self.calls.lock().expect("calls").push(Call::Begin);
        Ok(TransactionId::new("tx"))
    }

    async fn commit(&self, _transaction: &TransactionId) -> Result<String, TransportError> {
        self.calls.lock().expect("calls").push(Call::Commit);
        Ok("Transaction Committed".to_owned())
    }

    async fn rollback(&self, _transaction: &TransactionId) -> Result<(), TransportError> {
        self.calls.lock().expect("calls").push(Call::Rollback);
        Ok(())
    }
}

fn client(transport: &Arc<Counting>) -> DataApiClient {
    DataApiClient::new(
        transport.clone(),
        DataApiConfig::new(
            ResourceArn::parse("arn:aws:rds:eu-west-1:000000000000:cluster:aex").expect("arn"),
            SecretArn::parse("arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex-authz")
                .expect("arn"),
            DatabaseName::parse("aex").expect("database"),
        ),
    )
}

fn key_record() -> Vec<Field> {
    vec![
        Field::StringValue(Uuid::from_u128(1).to_string()),
        Field::StringValue(Uuid::from_u128(2).to_string()),
        Field::StringValue(Uuid::from_u128(3).to_string()),
        Field::ArrayValue(ArrayValue::StringValues(vec![Some(
            "sessions:read".to_owned(),
        )])),
        Field::BlobValue(Blob::new(vec![7_u8; 32])),
        Field::LongValue(1),
        Field::BooleanValue(false),
        Field::StringValue("eu-west-1".to_owned()),
        Field::StringValue("active".to_owned()),
        Field::StringValue("active".to_owned()),
        Field::StringValue("active".to_owned()),
        Field::LongValue(3),
        Field::LongValue(1),
        Field::LongValue(0),
    ]
}

fn central_actor_record(scopes: Vec<Option<String>>) -> Vec<Field> {
    vec![
        Field::StringValue(Uuid::from_u128(1).to_string()),
        Field::StringValue(Uuid::from_u128(2).to_string()),
        Field::ArrayValue(ArrayValue::StringValues(scopes)),
        Field::BlobValue(Blob::new(vec![7_u8; 32])),
        Field::LongValue(1),
        Field::BooleanValue(false),
        Field::BooleanValue(false),
        Field::BooleanValue(true),
        Field::StringValue("[]".to_owned()),
    ]
}

#[tokio::test]
async fn resolving_a_workspace_key_is_one_statement_and_no_transaction() {
    let transport = Counting::with(vec![key_record()]);
    let reader = AuroraAuthorizationReader::new(client(&transport));
    let resolved = reader
        .resolve_workspace_key(Uuid::from_u128(1))
        .await
        .expect("the read succeeds");
    assert!(resolved.is_some());
    assert_eq!(transport.statements(), 1, "the pinned per-refresh budget");
    assert_eq!(transport.transactions(), 0, "an assertion opens none");
}

#[tokio::test]
async fn resolving_a_session_for_a_workspace_is_also_one_statement() {
    let transport = Counting::with(Vec::new());
    let reader = AuroraAuthorizationReader::new(client(&transport));
    let resolved = reader
        .resolve_session_for_workspace(
            Uuid::from_u128(1),
            Uuid::from_u128(2),
            OffsetDateTime::UNIX_EPOCH,
        )
        .await
        .expect("the read succeeds");
    assert!(resolved.is_none());
    assert_eq!(transport.statements(), 1);
    assert_eq!(transport.transactions(), 0);
}

#[tokio::test]
async fn the_two_workspace_actor_statements_bind_the_same_parameters() {
    for (name, expected) in [
        ("token", sql::RESOLVE_ACCOUNT_TOKEN_FOR_WORKSPACE),
        ("session", sql::RESOLVE_SESSION_FOR_WORKSPACE),
    ] {
        let transport = Counting::with(Vec::new());
        let reader = AuroraAuthorizationReader::new(client(&transport));
        if name == "token" {
            reader
                .resolve_account_token_for_workspace(
                    Uuid::from_u128(1),
                    Uuid::from_u128(2),
                    OffsetDateTime::UNIX_EPOCH,
                )
                .await
                .expect("the read succeeds");
        } else {
            reader
                .resolve_session_for_workspace(
                    Uuid::from_u128(1),
                    Uuid::from_u128(2),
                    OffsetDateTime::UNIX_EPOCH,
                )
                .await
                .expect("the read succeeds");
        }
        let Some(Call::Execute { sql, parameters }) = transport.calls().first().cloned() else {
            panic!("{name} issued no statement");
        };
        assert_eq!(sql, expected, "{name}");
        assert_eq!(
            parameters,
            vec![
                "credential_id".to_owned(),
                "workspace_id".to_owned(),
                "now_ms".to_owned()
            ],
            "{name} binds the same three parameters"
        );
    }
}

#[tokio::test]
async fn both_central_actor_reads_are_one_statement_and_a_session_gets_only_bootstrap_scope() {
    for (session, expected) in [
        (false, sql::RESOLVE_ACCOUNT_TOKEN_CENTRAL),
        (true, sql::RESOLVE_SESSION_CENTRAL),
    ] {
        let scopes = if session {
            Vec::new()
        } else {
            vec![Some("account:read".to_owned())]
        };
        let transport = Counting::with(vec![central_actor_record(scopes)]);
        let reader = AuroraAuthorizationReader::new(client(&transport));
        let resolved = if session {
            reader
                .resolve_dashboard_session_central(Uuid::from_u128(1), OffsetDateTime::UNIX_EPOCH)
                .await
        } else {
            reader
                .resolve_account_token_central(Uuid::from_u128(1), OffsetDateTime::UNIX_EPOCH)
                .await
        }
        .expect("the read succeeds")
        .expect("the actor exists");
        assert_eq!(resolved.scopes.to_strings(), ["account:read"]);
        assert_eq!(transport.statements(), 1);
        assert_eq!(transport.transactions(), 0);
        let Some(Call::Execute { sql, parameters }) = transport.calls().first().cloned() else {
            panic!("the central actor read issued no statement");
        };
        assert_eq!(sql, expected);
        assert_eq!(parameters, ["credential_id", "now_ms"]);
    }
}

#[tokio::test]
async fn the_key_set_read_is_one_bounded_statement() {
    let transport = Counting::with(Vec::new());
    let reader = AuroraAuthorizationReader::new(client(&transport));
    reader
        .verification_key_set()
        .await
        .expect("the read succeeds");
    assert_eq!(transport.statements(), 1);
    assert!(sql::VERIFICATION_KEY_SET.contains("LIMIT 8"));
}

#[tokio::test]
async fn an_absent_active_signing_key_is_not_found_rather_than_an_empty_set() {
    let transport = Counting::with(Vec::new());
    let reader = AuroraAuthorizationReader::new(client(&transport));
    let error = reader
        .active_signing_key()
        .await
        .expect_err("a missing key is a readiness failure");
    assert_eq!(error, aex_control_app::ports::StoreError::NotFound);
}

// --- source discipline -------------------------------------------------------

/// Every statement this crate can issue.
fn statements() -> &'static [(&'static str, &'static str)] {
    sql::ALL
}

#[test]
fn no_statement_carries_a_format_placeholder() {
    for (name, statement) in statements() {
        assert!(
            !statement.contains("{}") && !statement.contains("{ }"),
            "`{name}` carries a format placeholder; SQL is never assembled at run time"
        );
    }
}

#[test]
fn every_parameter_is_named() {
    for (name, statement) in statements() {
        assert!(
            !statement.contains("$1") && !statement.contains('?'),
            "`{name}` uses a positional parameter; the Data API binds by name"
        );
    }
}

/// Whether `statement` binds a millisecond parameter, i.e. names `:<x>_ms`.
///
/// A projected `..._ms` **alias** is not a bound parameter; only a `:`-prefixed
/// name is, and conflating the two made an earlier version of this scan fire on
/// its own output column.
fn binds_millis(statement: &str) -> bool {
    statement
        .match_indices(':')
        .filter_map(|(at, _)| statement.get(at + 1..))
        .any(|tail| {
            let name: String = tail
                .chars()
                .take_while(|it| it.is_ascii_alphanumeric() || *it == '_')
                .collect();
            name.ends_with("_ms")
        })
}

/// Whether every occurrence of `column` in `projection` is either cast to epoch
/// milliseconds or reduced to a boolean.
///
/// `(k.revoked_at IS NOT NULL) AS key_revoked` projects a boolean, not an
/// instant, so it needs no cast; `s.expires_at AS expires_at` would.
fn timestamp_is_safely_projected(projection: &str, column: &str) -> bool {
    projection.match_indices(column).all(|(at, _)| {
        let before = &projection[at.saturating_sub(24)..at];
        let after = &projection[at + column.len()..projection.len().min(at + column.len() + 24)];
        before.contains("EXTRACT(EPOCH FROM ")
            || after.contains("IS NOT NULL")
            || after.contains("IS NULL")
            || after.starts_with(")*1000")
            || after.trim_start().starts_with("<=")
            || after.trim_start().starts_with('>')
            || after.trim_start().starts_with("_ms")
    })
}

/// The expression list that actually leaves the database, if this statement
/// returns rows. Insert/update column lists are not projections.
fn returned_projection(statement: &str) -> Option<&str> {
    if let Some((_, projection)) = statement.rsplit_once("RETURNING ") {
        return Some(projection);
    }
    statement
        .starts_with("SELECT ")
        .then(|| statement.split(" FROM ").next().unwrap_or_default())
}

#[test]
fn no_statement_projects_a_bare_timestamptz() {
    // A projected timestamp must go through the epoch-millis cast. The scan is
    // deliberately crude and deliberately loud: it looks for a timestamp column
    // name outside an `EXTRACT(EPOCH` and fails, because a naive timestamp
    // string is correct only while the session time zone happens to be UTC.
    for (name, statement) in statements() {
        let Some(projection) = returned_projection(statement) else {
            continue;
        };
        for column in [
            "retires_at",
            "created_at",
            "updated_at",
            "issued_at",
            "revoked_at",
            "activated_at",
            "deleted_at",
        ] {
            assert!(
                timestamp_is_safely_projected(projection, column),
                "`{name}` projects `{column}` without the epoch-millis cast"
            );
        }
    }
}

#[test]
fn every_timestamp_parameter_goes_through_the_millisecond_cast() {
    for (name, statement) in statements() {
        if !binds_millis(statement) {
            continue;
        }
        assert!(
            statement.contains("TIMESTAMPTZ 'epoch' +"),
            "`{name}` binds a millisecond parameter without the cast"
        );
    }
}

#[test]
fn every_collection_read_is_bounded() {
    for (name, statement) in statements() {
        let plural = statement.contains("ORDER BY");
        if plural {
            assert!(
                statement.contains("LIMIT"),
                "`{name}` orders rows without a bound"
            );
        }
    }
}

#[test]
fn the_write_probe_targets_a_table_the_read_only_role_cannot_write() {
    // The probe must be a real write against a real table: a probe that could
    // succeed for another reason would not prove the role is read-only.
    assert!(sql::AUTHZ_WRITE_PROBE.starts_with("INSERT INTO control.audit_event"));
}
