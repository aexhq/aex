//! Exact-session observation deletion admission and replay evidence.

use aex_observation_domain::keys::{self, ScopeKey};
use aex_observation_store_dynamodb::{
    SessionObservationDeletion, SessionObservationDeletionError, SessionObservationDeletionOutcome,
    SessionObservationDeletionRequest, SessionObservationDeletionStatus,
    SessionObservationDeletionStore,
};
use aex_wire::ids::{OperationId, SessionId, Uuid7, WorkspaceId};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};
use aws_smithy_http_client::test_util::{ReplayEvent, StaticReplayClient};
use aws_smithy_types::body::SdkBody;

struct Answer {
    status: u16,
    code: Option<&'static str>,
    body: String,
}

fn client(answers: Vec<Answer>) -> (Client, StaticReplayClient) {
    let events = answers
        .into_iter()
        .map(|answer| {
            let mut response = http::Response::builder().status(answer.status);
            if let Some(code) = answer.code {
                response = response.header("x-amzn-errortype", code);
            }
            ReplayEvent::new(
                http::Request::builder()
                    .method("POST")
                    .uri("https://dynamodb.eu-west-1.amazonaws.com/")
                    .body(SdkBody::empty())
                    .expect("request"),
                response.body(SdkBody::from(answer.body)).expect("response"),
            )
        })
        .collect();
    let replay = StaticReplayClient::new(events);
    let config = aws_sdk_dynamodb::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("eu-west-1"))
        .credentials_provider(Credentials::new(
            "AKIDTESTTESTTESTTEST",
            "test-secret",
            None,
            None,
            "aex-tests",
        ))
        .retry_config(aws_sdk_dynamodb::config::retry::RetryConfig::disabled())
        .http_client(replay.clone())
        .build();
    (Client::from_conf(config), replay)
}

fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [1; 10]))
}

fn session() -> SessionId {
    SessionId::from_uuid7(Uuid7::compose(1_754_051_696_789, [2; 10]))
}

fn operation() -> OperationId {
    OperationId::from_uuid7(Uuid7::compose(1_754_051_696_789, [3; 10]))
}

fn now() -> Timestamp {
    Timestamp::parse("2026-08-01T12:34:56.789Z").expect("timestamp")
}

fn request() -> SessionObservationDeletionRequest {
    SessionObservationDeletionRequest {
        workspace: workspace(),
        session: session(),
        operation: operation(),
        now: now(),
    }
}

fn requests(replay: &StaticReplayClient) -> Vec<serde_json::Value> {
    replay
        .actual_requests()
        .map(|request| {
            serde_json::from_slice(request.body().bytes().expect("request body"))
                .expect("request JSON")
        })
        .collect()
}

fn durable(status: &str, operation: OperationId) -> String {
    let scope = ScopeKey::Session {
        workspace: workspace(),
        session: session(),
    };
    serde_json::json!({
        "Item": {
            "pk": {"S": keys::frontier_pk(&scope)},
            "sk": {"S": keys::DELETION_SK},
            "itemType": {"S": "scope_deletion"},
            "scopeKey": {"S": scope.to_key()},
            "workspaceId": {"S": workspace().to_string()},
            "sessionId": {"S": session().to_string()},
            "operationId": {"S": operation.to_string()},
            "state": {"S": status},
            "deletionEpoch": {"N": "8"}
        }
    })
    .to_string()
}

fn conditional_failure() -> Answer {
    Answer {
        status: 400,
        code: Some("ConditionalCheckFailedException"),
        body: serde_json::json!({
            "__type": "com.amazonaws.dynamodb.v20120810#ConditionalCheckFailedException",
            "message": "fixture"
        })
        .to_string(),
    }
}

#[tokio::test]
async fn request_raises_or_increments_the_fence_and_enqueues_one_exact_duty() {
    let (client, replay) = client(vec![Answer {
        status: 200,
        code: None,
        body: "{}".to_owned(),
    }]);
    let store =
        SessionObservationDeletionStore::new(client, "observation-authority", 16).expect("store");

    assert_eq!(
        store.request(request()).await.expect("request"),
        SessionObservationDeletionOutcome::Started
    );
    let requests = requests(&replay);
    assert_eq!(requests.len(), 1);
    let wire = &requests[0];
    assert_eq!(wire["TableName"].as_str(), Some("observation-authority"));
    let update = wire["UpdateExpression"].as_str().expect("update");
    assert!(update.contains("if_not_exists"), "{update}");
    assert!(update.contains("cPk"), "{update}");
    assert!(update.contains("cSk"), "{update}");
    let condition = wire["ConditionExpression"].as_str().expect("condition");
    assert!(condition.contains("attribute_not_exists"), "{condition}");
    assert!(condition.contains(" = "), "{condition}");
    let strings: Vec<&str> = wire["ExpressionAttributeValues"]
        .as_object()
        .expect("values")
        .values()
        .filter_map(|value| value["S"].as_str())
        .collect();
    assert!(strings.contains(&"none"), "{strings:?}");
    assert!(strings.contains(&"deleting"), "{strings:?}");
    assert!(
        strings.contains(&operation().to_string().as_str()),
        "{strings:?}"
    );
    assert!(
        strings
            .iter()
            .any(|value| value.starts_with("CTRL#deletion.execute#")),
        "{strings:?}"
    );
}

#[tokio::test]
async fn a_lost_or_ambiguous_request_resolves_only_the_exact_durable_operation() {
    for failure in ["ConditionalCheckFailedException", "RequestTimeout"] {
        let first = if failure == "ConditionalCheckFailedException" {
            conditional_failure()
        } else {
            Answer {
                status: 400,
                code: Some("RequestTimeout"),
                body: serde_json::json!({
                    "__type": "com.amazonaws.dynamodb.v20120810#RequestTimeout",
                    "message": "fixture"
                })
                .to_string(),
            }
        };
        let (client, replay) = client(vec![
            first,
            Answer {
                status: 200,
                code: None,
                body: durable("verifying", operation()),
            },
        ]);
        let store = SessionObservationDeletionStore::new(client, "observation-authority", 16)
            .expect("store");
        assert_eq!(
            store.request(request()).await.expect("resolved request"),
            SessionObservationDeletionOutcome::Replay(SessionObservationDeletionStatus::Verifying),
            "{failure}"
        );
        let requests = requests(&replay);
        assert_eq!(requests.len(), 2, "{failure}");
        assert_eq!(requests[1]["ConsistentRead"].as_bool(), Some(true));
    }

    let other = OperationId::from_uuid7(Uuid7::compose(1_754_051_696_789, [9; 10]));
    let (client, _) = client(vec![
        conditional_failure(),
        Answer {
            status: 200,
            code: None,
            body: durable("deleting", other),
        },
    ]);
    let store =
        SessionObservationDeletionStore::new(client, "observation-authority", 16).expect("store");
    assert!(matches!(
        store.request(request()).await,
        Err(SessionObservationDeletionError::Conflict)
    ));
}

#[tokio::test]
async fn status_reports_only_a_proven_terminal_tombstone_for_the_coordinator() {
    let (client, replay) = client(vec![Answer {
        status: 200,
        code: None,
        body: durable("complete", operation()),
    }]);
    let store =
        SessionObservationDeletionStore::new(client, "observation-authority", 16).expect("store");
    assert_eq!(
        store
            .status(workspace(), session(), operation())
            .await
            .expect("status"),
        SessionObservationDeletionStatus::Complete
    );
    assert_eq!(requests(&replay)[0]["ConsistentRead"].as_bool(), Some(true));
}
