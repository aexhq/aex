//! Transport behaviour against a scripted Data `API` double.
//!
//! The double is the point: the budget guards, the transaction lifecycle and the
//! commit-ambiguity rule are all decisions this crate makes about a response, so
//! they are provable without an endpoint. What the double cannot prove — that
//! Aurora really appends `SQLState:` to its messages, and really rejects
//! concurrent use of a transaction id — is claimed by the live companion.

use std::sync::{Arc, Mutex};

use aex_rds_data::client::ExecuteResponse;
use aex_rds_data::{
    CommitFailure, DataApiClient, DataApiConfig, DataApiError, DatabaseName, DecodeError,
    ExceptionKind, Isolation, Record, ResourceArn, Row, SecretArn, SqlValue, Statement, Transport,
    TransportError,
};
use async_trait::async_trait;
use aws_sdk_rdsdata::types::{Field, SqlParameter};
use aws_smithy_types::Blob;

/// One programmed answer.
#[derive(Debug, Clone)]
enum Answer {
    Records(Vec<Vec<Field>>),
    Affected(u64),
    Fail(TransportError),
}

/// One recorded call.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Begin,
    Execute {
        sql: String,
        parameters: Vec<String>,
        transaction: Option<String>,
    },
    Commit,
    Rollback,
}

#[derive(Debug, Default)]
struct Script {
    answers: Vec<Answer>,
    commit: Option<Result<String, TransportError>>,
    calls: Vec<Call>,
}

#[derive(Debug, Default)]
struct Double(Mutex<Script>);

impl Double {
    fn with(answers: Vec<Answer>) -> Arc<Self> {
        Arc::new(Self(Mutex::new(Script {
            answers,
            commit: Some(Ok("Transaction Committed".to_owned())),
            calls: Vec::new(),
        })))
    }

    fn commit_answer(self: &Arc<Self>, answer: Result<String, TransportError>) {
        self.0.lock().expect("script").commit = Some(answer);
    }

    fn calls(self: &Arc<Self>) -> Vec<Call> {
        self.0.lock().expect("script").calls.clone()
    }

    fn statement_count(self: &Arc<Self>) -> usize {
        self.calls()
            .iter()
            .filter(|call| matches!(call, Call::Execute { .. }))
            .count()
    }
}

#[async_trait]
impl Transport for Double {
    async fn execute(
        &self,
        sql: &str,
        parameters: Vec<SqlParameter>,
        transaction: Option<&aex_rds_data::TransactionId>,
    ) -> Result<ExecuteResponse, TransportError> {
        let mut script = self.0.lock().expect("script");
        script.calls.push(Call::Execute {
            sql: sql.to_owned(),
            parameters: parameters
                .iter()
                .map(|it| it.name().unwrap_or_default().to_owned())
                .collect(),
            transaction: transaction.map(|it| it.as_str().to_owned()),
        });
        match script.answers.pop() {
            Some(Answer::Records(records)) => Ok(ExecuteResponse {
                records,
                rows_affected: 0,
            }),
            Some(Answer::Affected(rows)) => Ok(ExecuteResponse {
                records: Vec::new(),
                rows_affected: rows,
            }),
            Some(Answer::Fail(error)) => Err(error),
            None => Ok(ExecuteResponse::default()),
        }
    }

    async fn begin(&self) -> Result<aex_rds_data::TransactionId, TransportError> {
        self.0.lock().expect("script").calls.push(Call::Begin);
        Ok(aex_rds_data::TransactionId::new("tx-1"))
    }

    async fn commit(
        &self,
        _transaction: &aex_rds_data::TransactionId,
    ) -> Result<String, TransportError> {
        let mut script = self.0.lock().expect("script");
        script.calls.push(Call::Commit);
        script
            .commit
            .clone()
            .unwrap_or_else(|| Ok("Transaction Committed".to_owned()))
    }

    async fn rollback(
        &self,
        _transaction: &aex_rds_data::TransactionId,
    ) -> Result<(), TransportError> {
        self.0.lock().expect("script").calls.push(Call::Rollback);
        Ok(())
    }
}

fn config() -> DataApiConfig {
    DataApiConfig::new(
        ResourceArn::parse("arn:aws:rds:eu-west-1:000000000000:cluster:aex").expect("arn"),
        SecretArn::parse("arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex-x")
            .expect("arn"),
        DatabaseName::parse("aex").expect("database"),
    )
}

fn client(double: &Arc<Double>) -> DataApiClient {
    DataApiClient::new(double.clone(), config())
}

/// A two-column projection, the shape every repository row takes.
#[derive(Debug, PartialEq, Eq)]
struct Pair {
    id: uuid::Uuid,
    issued_at_ms: i64,
}

impl Row for Pair {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(2)?;
        Ok(Self {
            id: record.uuid(0)?,
            issued_at_ms: record.i64(1)?,
        })
    }
}

#[tokio::test]
async fn a_query_binds_named_parameters_and_decodes_rows() {
    let id = uuid::Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0001);
    let double = Double::with(vec![Answer::Records(vec![vec![
        Field::StringValue(id.to_string()),
        Field::LongValue(1_767_225_600_123),
    ]])]);
    let rows: Vec<Pair> = client(&double)
        .query(
            Statement::new("SELECT id, issued_at_ms FROM identity.user WHERE id = :id")
                .bind("id", SqlValue::Uuid(id)),
        )
        .await
        .expect("the query succeeds");
    assert_eq!(
        rows,
        vec![Pair {
            id,
            issued_at_ms: 1_767_225_600_123
        }]
    );
    assert_eq!(
        double.calls(),
        vec![Call::Execute {
            sql: "SELECT id, issued_at_ms FROM identity.user WHERE id = :id".to_owned(),
            parameters: vec!["id".to_owned()],
            transaction: None,
        }]
    );
}

#[tokio::test]
async fn a_double_value_anywhere_in_the_result_is_a_hard_decode_failure() {
    let double = Double::with(vec![Answer::Records(vec![vec![
        Field::StringValue(uuid::Uuid::nil().to_string()),
        Field::DoubleValue(1.5),
    ]])]);
    let error = client(&double)
        .query::<Pair>(Statement::new("SELECT id, issued_at_ms FROM identity.user"))
        .await
        .expect_err("a doubleValue never decodes");
    assert_eq!(
        error,
        DataApiError::Decode(DecodeError::UnexpectedDoubleValue { index: 1 })
    );
}

#[tokio::test]
async fn an_oversized_field_fails_before_the_row_decoder_runs() {
    let mut config = config();
    config.max_field_bytes = 8;
    let double = Double::with(vec![Answer::Records(vec![vec![Field::BlobValue(
        Blob::new(vec![0_u8; 9]),
    )]])]);
    let error = DataApiClient::new(double.clone(), config)
        .query::<Pair>(Statement::new(
            "SELECT verifier FROM identity.dashboard_session",
        ))
        .await
        .expect_err("the field budget is enforced");
    assert_eq!(error, DataApiError::FieldTooLarge { index: 0, bytes: 9 });
}

#[tokio::test]
async fn an_oversized_result_fails_before_the_row_decoder_runs() {
    let mut config = config();
    config.max_result_bytes = 16;
    let double = Double::with(vec![Answer::Records(vec![
        vec![Field::StringValue("x".repeat(10))],
        vec![Field::StringValue("y".repeat(10))],
    ])]);
    let error = DataApiClient::new(double.clone(), config)
        .query::<Pair>(Statement::new("SELECT name FROM control.workspace"))
        .await
        .expect_err("the result budget is enforced");
    assert_eq!(error, DataApiError::ResultTooLarge { bytes: 20 });
}

#[tokio::test]
async fn query_one_reports_an_arity_mismatch_rather_than_picking_a_row() {
    let double = Double::with(vec![Answer::Records(vec![
        vec![
            Field::StringValue(uuid::Uuid::nil().to_string()),
            Field::LongValue(1),
        ],
        vec![
            Field::StringValue(uuid::Uuid::nil().to_string()),
            Field::LongValue(2),
        ],
    ])]);
    let error = client(&double)
        .query_one::<Pair>(Statement::new("SELECT id, issued_at_ms FROM identity.user"))
        .await
        .expect_err("two rows is not one row");
    assert_eq!(
        error,
        DataApiError::Decode(DecodeError::ArityMismatch {
            expected: 1,
            actual: 2
        })
    );

    let empty = Double::with(vec![Answer::Records(Vec::new())]);
    let error = client(&empty)
        .query_one::<Pair>(Statement::new("SELECT id, issued_at_ms FROM identity.user"))
        .await
        .expect_err("zero rows is not one row");
    assert_eq!(
        error,
        DataApiError::Decode(DecodeError::ArityMismatch {
            expected: 1,
            actual: 0
        })
    );
}

#[tokio::test]
async fn a_serializable_transaction_sets_its_isolation_first() {
    let double = Double::with(vec![Answer::Affected(0), Answer::Affected(1)]);
    let client = client(&double);
    let mut transaction = client
        .begin(Isolation::Serializable)
        .await
        .expect("the transaction opens");
    transaction
        .execute(Statement::new(
            "UPDATE control.workspace SET revision = revision + 1",
        ))
        .await
        .expect("the statement runs");
    transaction.commit().await.expect("the commit confirms");

    let calls = double.calls();
    assert_eq!(calls[0], Call::Begin);
    assert!(matches!(
        &calls[1],
        Call::Execute { sql, .. } if sql == "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"
    ));
    assert!(matches!(
        &calls[2],
        Call::Execute { transaction: Some(id), .. } if id == "tx-1"
    ));
    assert_eq!(calls[3], Call::Commit);
}

#[tokio::test]
async fn a_read_committed_transaction_issues_no_isolation_statement() {
    let double = Double::with(vec![Answer::Affected(1)]);
    let client = client(&double);
    let transaction = client
        .begin(Isolation::ReadCommitted)
        .await
        .expect("the transaction opens");
    transaction.commit().await.expect("the commit confirms");
    assert_eq!(double.statement_count(), 0);
}

#[tokio::test]
async fn a_lost_commit_response_is_unknown_and_never_rolled_back() {
    let double = Double::with(Vec::new());
    double.commit_answer(Err(TransportError::Indeterminate {
        message: "connection reset".to_owned(),
    }));
    let client = client(&double);
    let transaction = client
        .begin(Isolation::ReadCommitted)
        .await
        .expect("the transaction opens");
    let failure = transaction
        .commit()
        .await
        .expect_err("a lost response is not a success");
    assert!(
        matches!(
            failure,
            CommitFailure::Unknown(DataApiError::Unavailable { .. })
        ),
        "{failure:?}"
    );
}

#[tokio::test]
async fn a_commit_timeout_is_unknown() {
    let double = Double::with(Vec::new());
    double.commit_answer(Err(TransportError::Service {
        kind: ExceptionKind::StatementTimeout,
        message: "timed out".to_owned(),
    }));
    let client = client(&double);
    let transaction = client
        .begin(Isolation::ReadCommitted)
        .await
        .expect("the transaction opens");
    assert_eq!(
        transaction.commit().await,
        Err(CommitFailure::Unknown(DataApiError::Timeout))
    );
}

#[tokio::test]
async fn a_deferred_constraint_raised_at_commit_is_a_definite_rollback() {
    let double = Double::with(Vec::new());
    double.commit_answer(Err(TransportError::Service {
        kind: ExceptionKind::BadRequest,
        message: "ERROR: organization 7c1f has no active owner; SQLState: 23000".to_owned(),
    }));
    let client = client(&double);
    let transaction = client
        .begin(Isolation::ReadCommitted)
        .await
        .expect("the transaction opens");
    let failure = transaction
        .commit()
        .await
        .expect_err("the deferred trigger aborted the transaction");
    assert!(
        matches!(
            failure,
            CommitFailure::RolledBack(DataApiError::IntegrityConstraintViolation { .. })
        ),
        "{failure:?}"
    );
}

#[tokio::test]
async fn a_reported_rollback_status_is_a_definite_rollback() {
    let double = Double::with(Vec::new());
    double.commit_answer(Ok("Rollback Complete".to_owned()));
    let client = client(&double);
    let transaction = client
        .begin(Isolation::ReadCommitted)
        .await
        .expect("the transaction opens");
    assert!(matches!(
        transaction.commit().await,
        Err(CommitFailure::RolledBack(_))
    ));
}

#[tokio::test]
async fn a_rollback_terminates_the_transaction() {
    let double = Double::with(Vec::new());
    let client = client(&double);
    let transaction = client
        .begin(Isolation::ReadCommitted)
        .await
        .expect("the transaction opens");
    transaction.rollback().await.expect("the rollback succeeds");
    assert_eq!(double.calls(), vec![Call::Begin, Call::Rollback]);
}

#[tokio::test]
async fn a_statement_failure_is_classified_through_the_error_table() {
    let double = Double::with(vec![Answer::Fail(TransportError::Service {
        kind: ExceptionKind::BadRequest,
        message:
            "ERROR: duplicate key value violates unique constraint \"org_slug_uk\"; SQLState: 23505"
                .to_owned(),
    })]);
    let error = client(&double)
        .execute(Statement::new(
            "INSERT INTO control.organization VALUES (1)",
        ))
        .await
        .expect_err("a unique violation is reported");
    assert_eq!(
        error,
        DataApiError::UniqueViolation {
            constraint: "org_slug_uk".to_owned()
        }
    );
}

#[tokio::test]
async fn one_authorization_read_is_exactly_one_statement_and_no_transaction() {
    let id = uuid::Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_00ff);
    let double = Double::with(vec![Answer::Records(vec![vec![
        Field::StringValue(id.to_string()),
        Field::LongValue(3),
    ]])]);
    let _row: Option<Pair> = client(&double)
        .query_opt(
            Statement::new("SELECT k.id, epoch_key FROM control.api_key k WHERE k.id = :key_id")
                .bind("key_id", SqlValue::Uuid(id)),
        )
        .await
        .expect("the authorization read succeeds");
    assert_eq!(double.statement_count(), 1);
    assert!(
        !double
            .calls()
            .iter()
            .any(|call| matches!(call, Call::Begin | Call::Commit | Call::Rollback)),
        "an assertion issue opens no transaction"
    );
}
