//! The `SQS`-backed [`WakeQueue`], and the `DynamoDB` due scan behind it.
//!
//! The queue is a **delivery hint**. It has no `enqueue`, because a wake exists only inside
//! a `DecisionCommit`; if the queue could originate one it would be a second authority, and
//! two authorities for "what should run next" is how work gets invented and lost.
//!
//! Delivery is at-least-once. Visibility is not a lease and `DeleteMessage` is not a
//! commit: the ack happens strictly after the decision commits, so a crash between them
//! redelivers rather than loses.

use aex_brain_application::ports::{BoxFuture, DurableWake, StoreError, WakeDelivery, WakeQueue};
use aex_brain_domain::ids::{AgentId, AgentKey, SessionId, Timestamp, WakeId, WorkShard};
use aex_brain_domain::journal::ParkReason;
use aws_sdk_sqs::Client as SqsClient;
use aws_sdk_sqs::types::MessageSystemAttributeName;

/// The long-poll window one receive waits.
pub const LONG_POLL: core::time::Duration = core::time::Duration::from_secs(20);

/// The base visibility a delivery is given.
pub const BASE_VISIBILITY: core::time::Duration = core::time::Duration::from_secs(30);

/// The most deliveries one receive returns.
pub const MAX_BATCH: usize = 10;

/// The `SQS` wake queue.
#[derive(Debug, Clone)]
pub struct SqsWakeQueue {
    client: SqsClient,
    queue_url: String,
    due: DueScan,
}

/// Where the due backstop reads outstanding work from.
///
/// The reconciler reads the durable due index rather than the queue, because it exists
/// precisely for the case where the stream projection into the queue was delayed or lost.
#[derive(Debug, Clone)]
pub struct DueScan {
    client: aws_sdk_dynamodb::Client,
    table: String,
}

impl DueScan {
    /// Binds the scan to the `regional-work` table.
    #[must_use]
    pub const fn new(client: aws_sdk_dynamodb::Client, table: String) -> Self {
        Self { client, table }
    }
}

impl SqsWakeQueue {
    /// Binds the queue to its URL and its due backstop.
    #[must_use]
    pub const fn new(client: SqsClient, queue_url: String, due: DueScan) -> Self {
        Self {
            client,
            queue_url,
            due,
        }
    }

    /// The queue this adapter drains.
    #[must_use]
    pub fn queue_url(&self) -> &str {
        &self.queue_url
    }
}

impl WakeQueue for SqsWakeQueue {
    fn receive(
        &self,
        max: usize,
        wait: core::time::Duration,
    ) -> BoxFuture<'_, Result<Vec<WakeDelivery>, StoreError>> {
        Box::pin(async move {
            let output = self
                .client
                .receive_message()
                .queue_url(&self.queue_url)
                .max_number_of_messages(i32::try_from(max.min(MAX_BATCH)).unwrap_or(1))
                .wait_time_seconds(i32::try_from(wait.as_secs()).unwrap_or(20).min(20))
                .message_system_attribute_names(MessageSystemAttributeName::ApproximateReceiveCount)
                .send()
                .await
                .map_err(|error| sqs_error("receive", &error))?;
            output
                .messages
                .unwrap_or_default()
                .into_iter()
                .map(decode_message)
                .collect()
        })
    }

    fn extend_visibility<'a>(
        &'a self,
        delivery: &'a WakeDelivery,
        by: core::time::Duration,
    ) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(async move {
            self.client
                .change_message_visibility()
                .queue_url(&self.queue_url)
                .receipt_handle(&delivery.receipt)
                .visibility_timeout(i32::try_from(by.as_secs()).unwrap_or(30))
                .send()
                .await
                .map_err(|error| sqs_error("extend_visibility", &error))?;
            Ok(())
        })
    }

    fn release(
        &self,
        delivery: WakeDelivery,
        after: core::time::Duration,
    ) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(async move {
            self.client
                .change_message_visibility()
                .queue_url(&self.queue_url)
                .receipt_handle(delivery.receipt)
                .visibility_timeout(i32::try_from(after.as_secs()).unwrap_or(0))
                .send()
                .await
                .map_err(|error| sqs_error("release", &error))?;
            Ok(())
        })
    }

    fn ack(&self, delivery: WakeDelivery) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(async move {
            self.client
                .delete_message()
                .queue_url(&self.queue_url)
                .receipt_handle(delivery.receipt)
                .send()
                .await
                .map_err(|error| sqs_error("ack", &error))?;
            Ok(())
        })
    }

    fn due_scan(
        &self,
        shard: WorkShard,
        now: Timestamp,
        max: usize,
    ) -> BoxFuture<'_, Result<Vec<DurableWake>, StoreError>> {
        Box::pin(async move {
            let due_before =
                aex_wire::types::Timestamp::from_unix_millis(now.millis()).map_err(|_| {
                    StoreError::Undecodable {
                        location: "due scan".to_owned(),
                        reason: "the scan instant is outside the wire range".to_owned(),
                    }
                })?;
            let output = self
                .due
                .client
                .query()
                .table_name(&self.due.table)
                .index_name(aex_work_dynamodb::keys::DUE_INDEX)
                .key_condition_expression("dueShardPk = :shard AND dueShardSk <= :due")
                .expression_attribute_values(
                    ":shard",
                    aex_session_dynamodb::attr::s(
                        aex_work_dynamodb::keys::due_partition_for_shard(shard.0),
                    ),
                )
                .expression_attribute_values(
                    ":due",
                    aex_session_dynamodb::attr::s(due_before.to_wire()),
                )
                .limit(i32::try_from(max).unwrap_or(50))
                .send()
                .await
                .map_err(|error| StoreError::Transport {
                    reason: format!("due_scan: {error}"),
                    retryable: true,
                })?;
            Ok(output.items().iter().filter_map(decode_due_entry).collect())
        })
    }
}

/// Decodes one queue message into a durable wake.
///
/// The message body is the projection of the durable row, so a message that does not carry
/// the agent it names is dropped rather than guessed at: an invented agent key would wake
/// something that was never asked to run.
///
/// # Errors
///
/// [`StoreError::Undecodable`] when the message carries no receipt handle or no body.
pub fn decode_message(message: aws_sdk_sqs::types::Message) -> Result<WakeDelivery, StoreError> {
    let receipt = message
        .receipt_handle
        .ok_or_else(|| StoreError::Undecodable {
            location: "wake delivery".to_owned(),
            reason: "the message carried no receipt handle".to_owned(),
        })?;
    let body = message.body.ok_or_else(|| StoreError::Undecodable {
        location: "wake delivery".to_owned(),
        reason: "the message carried no body".to_owned(),
    })?;
    let receive_count = message
        .attributes
        .as_ref()
        .and_then(|it| it.get(&MessageSystemAttributeName::ApproximateReceiveCount))
        .and_then(|it| it.parse::<u32>().ok())
        .unwrap_or(1);
    let wake = decode_body(&body)?;
    Ok(WakeDelivery {
        wake,
        receipt,
        receive_count,
    })
}

/// Decodes the wake payload a `regional-work` row projects onto the queue.
///
/// # Errors
///
/// [`StoreError::Undecodable`] when the payload is not JSON or names no agent.
pub fn decode_body(body: &str) -> Result<DurableWake, StoreError> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|error| StoreError::Undecodable {
            location: "wake payload".to_owned(),
            reason: error.to_string(),
        })?;
    let payload = value.get("payload").unwrap_or(&value);
    let session = parse_id(payload.get("sessionId"), "sessionId")?;
    let agent = parse_id(payload.get("agentId"), "agentId")?;
    let work_id = value
        .get("workId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    Ok(DurableWake {
        id: WakeId(wake_identity(work_id)),
        key: AgentKey::new(SessionId(session), AgentId(agent)),
        dedup_key: work_id.to_owned(),
        // The reason is a scheduling hint. What the agent actually owes is a total function
        // of its folded state, so a wake that lies about the reason cannot make it act
        // wrongly; it only makes the diagnostic wrong.
        reason: ParkReason::AwaitingUserMessage,
        due: value
            .get("dueAt")
            .and_then(serde_json::Value::as_str)
            .and_then(|text| text.parse::<i64>().ok())
            .map(Timestamp::from_millis),
        priority: value
            .get("priority")
            .and_then(serde_json::Value::as_u64)
            .and_then(|it| u8::try_from(it).ok())
            .unwrap_or(1),
        tenant: value
            .get("workspaceId")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    })
}

/// The wake identity a durable work row projects to.
///
/// Derived from the work identity rather than minted, so two deliveries of one row collapse
/// locally before either reaches admission. A fresh identity per delivery would make the
/// local dedup map useless.
fn wake_identity(work_id: &str) -> uuid::Uuid {
    let digest = blake3::hash(work_id.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    bytes[6] = (bytes[6] & 0x0F) | 0x40;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    uuid::Uuid::from_bytes(bytes)
}

fn parse_id(value: Option<&serde_json::Value>, what: &str) -> Result<uuid::Uuid, StoreError> {
    let text =
        value
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| StoreError::Undecodable {
                location: "wake payload".to_owned(),
                reason: format!("`{what}` is absent"),
            })?;
    // Either prefix is accepted because the payload carries both a session and an agent id
    // and this reads both; a wrong-kind id would simply fail to parse as either.
    if let Ok(id) = text.parse::<aex_wire::ids::SessionId>() {
        return Ok(uuid::Uuid::from_bytes(
            *aex_wire::ids::PrefixedId::uuid7(&id).as_bytes(),
        ));
    }
    let agent: aex_wire::ids::AgentId = text.parse().map_err(|_| StoreError::Undecodable {
        location: "wake payload".to_owned(),
        reason: format!("`{what}` is not a prefixed identifier"),
    })?;
    Ok(uuid::Uuid::from_bytes(
        *aex_wire::ids::PrefixedId::uuid7(&agent).as_bytes(),
    ))
}

fn decode_due_entry(item: &aex_session_dynamodb::attr::Item) -> Option<DurableWake> {
    let work_id = item.get("workId")?.as_s().ok()?;
    Some(DurableWake {
        id: WakeId(wake_identity(work_id)),
        key: AgentKey::new(SessionId(uuid::Uuid::nil()), AgentId(uuid::Uuid::nil())),
        dedup_key: work_id.clone(),
        reason: ParkReason::AwaitingUserMessage,
        due: None,
        priority: 1,
        tenant: String::new(),
    })
}

fn sqs_error<E, R>(operation: &str, error: &aws_sdk_sqs::error::SdkError<E, R>) -> StoreError
where
    E: aws_smithy_types::error::metadata::ProvideErrorMetadata,
{
    // Every queue operation is retryable by construction: the durable wake survives a
    // failed receive, a failed visibility change and a failed ack alike, so a transport
    // failure here costs a redelivery rather than a lost unit of work.
    StoreError::Transport {
        reason: format!("{operation}: {error}"),
        retryable: true,
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_body, decode_message};
    use aex_wire::ids::{PrefixedId, Uuid7};

    fn body() -> String {
        let session =
            aex_wire::ids::SessionId::from_uuid7(Uuid7::compose(1_767_225_600_000, [1; 10]));
        let agent = aex_wire::ids::AgentId::from_uuid7(Uuid7::compose(1_767_225_600_001, [2; 10]));
        format!(
            r#"{{"workId":"wrk_1","priority":2,"workspaceId":"ws_x","payload":{{"sessionId":"{session}","agentId":"{agent}"}}}}"#
        )
    }

    #[test]
    fn a_projected_row_decodes_into_the_agent_it_names() {
        let wake = decode_body(&body()).expect("a well-formed projection");
        assert_eq!(wake.dedup_key, "wrk_1");
        assert_eq!(wake.priority, 2);
        assert_eq!(wake.tenant, "ws_x");
    }

    /// The wake identity is derived from the durable work identity, so two deliveries of
    /// one row collapse locally before either reaches admission.
    #[test]
    fn two_deliveries_of_one_row_carry_the_same_wake_identity() {
        let first = decode_body(&body()).expect("well-formed");
        let second = decode_body(&body()).expect("well-formed");
        assert_eq!(first.id, second.id);
    }

    #[test]
    fn a_payload_naming_no_agent_is_refused_rather_than_guessed() {
        let error = decode_body(r#"{"workId":"wrk_1","payload":{}}"#)
            .expect_err("an invented agent key would wake something nobody asked to run");
        assert!(format!("{error}").contains("agentId") || format!("{error}").contains("sessionId"));
    }

    #[test]
    fn a_message_with_no_receipt_handle_is_refused() {
        let message = aws_sdk_sqs::types::Message::builder().body(body()).build();
        assert!(decode_message(message).is_err());
    }

    #[test]
    fn a_receive_count_is_carried_so_the_poison_policy_can_see_it() {
        let message = aws_sdk_sqs::types::Message::builder()
            .body(body())
            .receipt_handle("rh")
            .attributes(
                aws_sdk_sqs::types::MessageSystemAttributeName::ApproximateReceiveCount,
                "7",
            )
            .build();
        let delivery = decode_message(message).expect("well-formed");
        assert_eq!(delivery.receive_count, 7);
    }
}
