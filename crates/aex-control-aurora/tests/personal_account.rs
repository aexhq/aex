//! Transaction-shape and replay proof for first-login provisioning.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use aws_sdk_rdsdata::types::{Field, SqlParameter};
use time::OffsetDateTime;
use uuid::{Builder, Uuid};

use aex_control_app::ports::IdFactory;
use aex_control_app::{PersonalAccountProvisioner, ProvisionPersonalAccount};
use aex_control_aurora::{AuroraControlStore, sql};
use aex_rds_data::client::ExecuteResponse;
use aex_rds_data::{
    DataApiClient, DataApiConfig, DatabaseName, ExceptionKind, ResourceArn, SecretArn,
    TransactionId, Transport, TransportError,
};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Begin,
    Execute(String),
    Commit,
    Rollback,
}

#[derive(Debug)]
struct ScriptedTransport {
    responses: Mutex<VecDeque<ExecuteResponse>>,
    calls: Mutex<Vec<Call>>,
    fail_statement: Option<usize>,
}

impl ScriptedTransport {
    fn new(responses: Vec<ExecuteResponse>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            calls: Mutex::new(Vec::new()),
            fail_statement: None,
        })
    }

    fn failing(responses: Vec<ExecuteResponse>, fail_statement: usize) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            calls: Mutex::new(Vec::new()),
            fail_statement: Some(fail_statement),
        })
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().expect("calls").clone()
    }
}

#[async_trait]
impl Transport for ScriptedTransport {
    async fn execute(
        &self,
        statement: &str,
        _parameters: Vec<SqlParameter>,
        _transaction: Option<&TransactionId>,
    ) -> Result<ExecuteResponse, TransportError> {
        let mut calls = self.calls.lock().expect("calls");
        let statement_number = calls
            .iter()
            .filter(|call| matches!(call, Call::Execute(_)))
            .count();
        calls.push(Call::Execute(statement.to_owned()));
        drop(calls);
        if self.fail_statement == Some(statement_number) {
            return Err(TransportError::Service {
                kind: ExceptionKind::DatabaseError,
                message: "SQLSTATE 23503 injected write refusal".to_owned(),
            });
        }
        Ok(self
            .responses
            .lock()
            .expect("responses")
            .pop_front()
            .expect("one response per statement"))
    }

    async fn begin(&self) -> Result<TransactionId, TransportError> {
        self.calls.lock().expect("calls").push(Call::Begin);
        Ok(TransactionId::new("personal-account"))
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

#[tokio::test]
async fn a_failed_write_rolls_back_the_whole_first_login_aggregate() {
    let user = Uuid::now_v7();
    let transport = ScriptedTransport::failing(
        vec![
            response(vec![vec![Field::StringValue(String::new())]]),
            response(Vec::new()),
            response(Vec::new()),
        ],
        3,
    );
    let store = AuroraControlStore::new(client(&transport));

    ProvisionPersonalAccount::run(
        &store as &dyn PersonalAccountProvisioner,
        &OrderedIds::new(),
        user,
        OffsetDateTime::UNIX_EPOCH,
    )
    .await
    .expect_err("the injected membership failure refuses first login");

    let calls = transport.calls();
    assert_eq!(calls.last(), Some(&Call::Rollback));
    assert!(!calls.contains(&Call::Commit));
    assert!(
        !calls.iter().any(|call| matches!(call, Call::Execute(statement) if statement == sql::INSERT_PERSONAL_ACCOUNT)),
        "no aggregate marker can survive a failed prerequisite"
    );
}

struct OrderedIds(Mutex<u64>);

impl OrderedIds {
    fn new() -> Self {
        Self(Mutex::new(0))
    }
}

impl IdFactory for OrderedIds {
    fn next(&self) -> Uuid {
        let mut issued = self.0.lock().expect("ids");
        let id =
            Builder::from_unix_timestamp_millis(1_700_000_000_000 + *issued, &[0; 10]).into_uuid();
        *issued += 1;
        id
    }
}

fn client(transport: &Arc<ScriptedTransport>) -> DataApiClient {
    DataApiClient::new(
        transport.clone(),
        DataApiConfig::new(
            ResourceArn::parse("arn:aws:rds:eu-west-1:000000000000:cluster:aex").expect("arn"),
            SecretArn::parse("arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex-control")
                .expect("arn"),
            DatabaseName::parse("aex").expect("database"),
        ),
    )
}

fn response(records: Vec<Vec<Field>>) -> ExecuteResponse {
    ExecuteResponse {
        records,
        rows_affected: 1,
    }
}

fn aggregate(ids: &[Uuid], user: Uuid) -> Vec<Field> {
    vec![
        Field::StringValue(ids[0].to_string()),
        Field::StringValue(user.to_string()),
        Field::StringValue(ids[1].to_string()),
        Field::StringValue(ids[2].to_string()),
        Field::StringValue(ids[7].to_string()),
        Field::StringValue(ids[8].to_string()),
        Field::LongValue(0),
    ]
}

#[tokio::test]
async fn first_login_is_one_transaction_with_outbox_last_and_no_partial_commit() {
    let user = Builder::from_unix_timestamp_millis(1_699_999_999_999, &[1; 10]).into_uuid();
    let expected: Vec<Uuid> = (0_u64..9)
        .map(|offset| {
            Builder::from_unix_timestamp_millis(1_700_000_000_000 + offset, &[0; 10]).into_uuid()
        })
        .collect();
    let mut responses = vec![
        response(vec![vec![Field::StringValue(String::new())]]),
        response(Vec::new()),
    ];
    responses.extend((0..2).map(|_| response(Vec::new())));
    responses.push(response(vec![vec![Field::StringValue(String::new())]]));
    responses.extend((0..7).map(|_| response(Vec::new())));
    responses.push(response(vec![aggregate(&expected, user)]));
    let transport = ScriptedTransport::new(responses);
    let store = AuroraControlStore::new(client(&transport));

    let provisioned = ProvisionPersonalAccount::run(
        &store as &dyn PersonalAccountProvisioner,
        &OrderedIds::new(),
        user,
        OffsetDateTime::UNIX_EPOCH,
    )
    .await
    .expect("first login commits");
    assert_eq!(provisioned.account_id, expected[0]);
    assert_eq!(provisioned.workspace_id, expected[2]);

    let calls = transport.calls();
    assert_eq!(calls.first(), Some(&Call::Begin));
    assert_eq!(calls.last(), Some(&Call::Commit));
    assert!(!calls.contains(&Call::Rollback));
    let statements: Vec<&str> = calls
        .iter()
        .filter_map(|call| match call {
            Call::Execute(statement) => Some(statement.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(statements[0], sql::LOCK_PERSONAL_ACCOUNT);
    assert_eq!(statements[1], sql::GET_PERSONAL_ACCOUNT);
    assert_eq!(statements[statements.len() - 2], sql::INSERT_OUTBOX);
    assert_eq!(statements.last(), Some(&sql::GET_PERSONAL_ACCOUNT));
}

#[tokio::test]
async fn retry_after_response_loss_reads_the_winner_and_writes_nothing() {
    let user = Uuid::now_v7();
    let stored: Vec<Uuid> = (0..9).map(|_| Uuid::now_v7()).collect();
    let transport = ScriptedTransport::new(vec![
        response(vec![vec![Field::StringValue(String::new())]]),
        response(vec![aggregate(&stored, user)]),
    ]);
    let store = AuroraControlStore::new(client(&transport));

    let replayed = ProvisionPersonalAccount::run(
        &store as &dyn PersonalAccountProvisioner,
        &OrderedIds::new(),
        user,
        OffsetDateTime::UNIX_EPOCH,
    )
    .await
    .expect("winner replays");
    assert_eq!(replayed.account_id, stored[0]);
    assert_eq!(replayed.workspace_id, stored[2]);
    assert_eq!(
        transport.calls(),
        vec![
            Call::Begin,
            Call::Execute(sql::LOCK_PERSONAL_ACCOUNT.to_owned()),
            Call::Execute(sql::GET_PERSONAL_ACCOUNT.to_owned()),
            Call::Rollback,
        ]
    );
}
