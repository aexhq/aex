//! Capacity-limit producer request, ambiguity and denial evidence.
//!
//! This target requires only `capacity-limit-projection-write`; the focused
//! command runs it with default features disabled so the producer cannot rely
//! on the regional query surface or central control writer.

use aex_session_dynamodb::StoreError;
use aex_session_dynamodb::capacity_limit_projection_write::{
    CapacityLimitProjectionWriter, LimitWrite,
};
use aex_wire::ids::{PrefixedId as _, Uuid7, WorkspaceId};
use aex_wire::limits::LimitId;
use aex_wire::models::{LimitScalarValue, LimitSource, LimitValue};
use aex_wire::types::{DecimalU128, Timestamp};
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
                    .expect("a request"),
                response
                    .body(SdkBody::from(answer.body))
                    .expect("a response"),
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

fn write() -> LimitWrite {
    LimitWrite {
        workspace: WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10])),
        id: LimitId::QueryPage,
        effective_value: LimitValue::Scalar(LimitScalarValue {
            value: DecimalU128::new(1_000),
        }),
        source: LimitSource::Default,
        revision: 3,
        changed_at: Timestamp::from_unix_millis(1_000).expect("timestamp"),
    }
}

fn service_error(code: &'static str) -> Answer {
    Answer {
        status: 400,
        code: Some(code),
        body: serde_json::json!({
            "__type": format!("com.amazonaws.dynamodb.v20120810#{code}"),
            "message": "fixture"
        })
        .to_string(),
    }
}

fn durable_item(write: &LimitWrite, limit_id: LimitId) -> String {
    let value = serde_json::to_string(&write.effective_value).expect("limit value encodes");
    serde_json::json!({
        "Item": {
            "pk": {"S": format!("LIMIT#WS#{}", write.workspace)},
            "sk": {"S": format!("LIMIT#{}", write.id.as_str())},
            "itemType": {"S": "workspace_limit"},
            "workspaceId": {"S": write.workspace.to_string()},
            "limitId": {"S": limit_id.as_str()},
            "effectiveValue": {"S": value},
            "source": {"S": write.source.as_str()},
            "revision": {"N": write.revision.to_string()},
            "changedAt": {"S": write.changed_at.to_wire()}
        }
    })
    .to_string()
}

fn requests(replay: &StaticReplayClient) -> Vec<serde_json::Value> {
    replay
        .actual_requests()
        .map(|request| {
            serde_json::from_slice(
                request
                    .body()
                    .bytes()
                    .expect("the DynamoDB request body is in memory"),
            )
            .expect("the request body is JSON")
        })
        .collect()
}

#[tokio::test]
async fn the_wire_condition_binds_identity_and_uses_the_capacity_partition() {
    let (client, replay) = client(vec![Answer {
        status: 200,
        code: None,
        body: "{}".to_owned(),
    }]);
    let write = write();
    CapacityLimitProjectionWriter::new(client, "projection")
        .put_limit(&write)
        .await
        .expect("the write succeeds");

    let requests = requests(&replay);
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    let condition = request["ConditionExpression"]
        .as_str()
        .expect("a condition expression");
    for identity in ["#item_type", "#workspace_id", "#limit_id"] {
        assert!(condition.contains(identity), "condition omitted {identity}");
    }
    assert_eq!(
        request["Item"]["pk"]["S"].as_str(),
        Some(format!("LIMIT#WS#{}", write.workspace).as_str())
    );
    assert_eq!(
        request["ExpressionAttributeValues"][":item_type"]["S"].as_str(),
        Some("workspace_limit")
    );
}

#[tokio::test]
async fn conditional_and_commit_ambiguous_outcomes_use_one_strong_resolution_read() {
    for failure in ["ConditionalCheckFailedException", "RequestTimeout"] {
        let write = write();
        let (client, replay) = client(vec![
            service_error(failure),
            Answer {
                status: 200,
                code: None,
                body: durable_item(&write, write.id),
            },
        ]);
        CapacityLimitProjectionWriter::new(client, "projection")
            .put_limit(&write)
            .await
            .unwrap_or_else(|error| panic!("{failure} did not resolve: {error}"));
        let requests = requests(&replay);
        assert_eq!(requests.len(), 2, "{failure}");
        assert_eq!(requests[1]["ConsistentRead"].as_bool(), Some(true));
    }
}

#[tokio::test]
async fn a_provider_500_performs_one_strong_target_read_and_no_second_write() {
    let write = write();
    let (client, replay) = client(vec![
        Answer {
            status: 500,
            code: Some("InternalServerError"),
            body: serde_json::json!({
                "__type": "com.amazonaws.dynamodb.v20120810#InternalServerError",
                "message": "fixture"
            })
            .to_string(),
        },
        Answer {
            status: 200,
            code: None,
            body: durable_item(&write, write.id),
        },
    ]);

    CapacityLimitProjectionWriter::new(client, "projection")
        .put_limit(&write)
        .await
        .expect("the strong target read observes the committed write");

    let requests = requests(&replay);
    assert_eq!(requests.len(), 2, "one write and one resolver read");
    assert!(
        requests[0].get("Item").is_some(),
        "the first call is PutItem"
    );
    assert!(
        requests[1].get("Item").is_none(),
        "the second call is not another PutItem"
    );
    assert_eq!(requests[1]["ConsistentRead"].as_bool(), Some(true));
}

#[tokio::test]
async fn denial_throttle_validation_and_missing_table_return_without_a_read() {
    for (code, expected) in [
        ("AccessDeniedException", "denied"),
        ("ProvisionedThroughputExceededException", "throttled"),
        ("ValidationException", "invalid"),
        ("ResourceNotFoundException", "misconfigured"),
    ] {
        let (client, replay) = client(vec![service_error(code)]);
        let error = CapacityLimitProjectionWriter::new(client, "projection")
            .put_limit(&write())
            .await
            .expect_err("the definitive failure returns");
        match expected {
            "denied" => assert!(matches!(error, StoreError::Denied), "{code}: {error}"),
            "throttled" => {
                assert!(
                    matches!(error, StoreError::Throttled { .. }),
                    "{code}: {error}"
                );
            }
            "invalid" => assert!(
                matches!(error, StoreError::Invalid { .. }),
                "{code}: {error}"
            ),
            "misconfigured" => {
                assert!(
                    matches!(error, StoreError::Misconfigured { .. }),
                    "{code}: {error}"
                );
            }
            _ => unreachable!(),
        }
        assert_eq!(requests(&replay).len(), 1, "{code} triggered a read");
    }
}

#[tokio::test]
async fn a_resolution_read_fails_closed_on_stored_identity_corruption() {
    let write = write();
    let (client, replay) = client(vec![
        service_error("ConditionalCheckFailedException"),
        Answer {
            status: 200,
            code: None,
            body: durable_item(&write, LimitId::RequestBodyBytes),
        },
    ]);
    let error = CapacityLimitProjectionWriter::new(client, "projection")
        .put_limit(&write)
        .await
        .expect_err("foreign durable identity is corruption");
    assert!(matches!(error, StoreError::Corrupt(_)), "{error}");
    assert_eq!(requests(&replay).len(), 2);
}
