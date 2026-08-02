//! The `SQS`-backed [`WakeQueue`], and the `DynamoDB` due scan behind it.
//!
//! The queue is a **delivery hint**. It has no `enqueue`, because a wake exists only inside
//! a `DecisionCommit`; if the queue could originate one it would be a second authority, and
//! two authorities for "what should run next" is how work gets invented and lost.
//!
//! Delivery is at-least-once. Visibility is not a lease and `DeleteMessage` is not a
//! commit: the ack happens strictly after the decision commits, so a crash between them
//! redelivers rather than loses.

use aex_brain_application::ports::{
    BoxFuture, DurableWake, StoreError, WakeDelivery, WakeOrigin, WakeQueue, WakeState,
};
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

    fn state<'a>(&'a self, wake: &'a DurableWake) -> BoxFuture<'a, Result<WakeState, StoreError>> {
        Box::pin(async move {
            let workspace = parse_workspace(&wake.tenant)?;
            let source_key = aex_work_dynamodb::keys::work(&wake.work_id)
                .map_err(|error| undecodable("regional-work/source wake", error.to_string()))?;
            let output = self
                .due
                .client
                .get_item()
                .table_name(&self.due.table)
                .set_key(Some(aex_session_dynamodb::plan::key(
                    &source_key.pk,
                    &source_key.sk,
                )))
                .consistent_read(true)
                .send()
                .await
                .map_err(|error| StoreError::Transport {
                    reason: format!("wake_state: {error}"),
                    retryable: true,
                })?;
            let Some(item) = output.item else {
                return Ok(WakeState::Retired);
            };
            let record = aex_work_dynamodb::codec::decode_work(&item, workspace)
                .map_err(|error| undecodable("regional-work/source wake", error.to_string()))?;
            let (session, agent) = crate::translate::agent_key(&wake.key)
                .map_err(|error| undecodable("regional-work/source wake", error.to_string()))?;
            if record.work_id != wake.work_id
                || record.kind != "agent.wake"
                || record.workspace != workspace
                || record.session != Some(session)
                || record.agent != Some(agent)
                || wake.id != WakeId(wake_identity(&record.work_id))
                || record.due_at.unix_millis()
                    != wake.due.map(Timestamp::millis).unwrap_or_default()
                || record.priority != wake.priority
            {
                return Err(undecodable(
                    "regional-work/source wake",
                    "the delivery does not match the authoritative agent wake",
                ));
            }
            match record.state.as_str() {
                "pending" => Ok(WakeState::Pending),
                "done" | "poisoned" => Ok(WakeState::Retired),
                other => Err(undecodable(
                    "regional-work/source wake",
                    format!("agent wake is in unsupported state `{other}`"),
                )),
            }
        })
    }

    fn extend_visibility<'a>(
        &'a self,
        delivery: &'a WakeDelivery,
        by: core::time::Duration,
    ) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(async move {
            let WakeOrigin::Queue { receipt, .. } = &delivery.origin else {
                return Ok(());
            };
            self.client
                .change_message_visibility()
                .queue_url(&self.queue_url)
                .receipt_handle(receipt)
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
            let WakeOrigin::Queue { receipt, .. } = delivery.origin else {
                return Ok(());
            };
            self.client
                .change_message_visibility()
                .queue_url(&self.queue_url)
                .receipt_handle(receipt)
                .visibility_timeout(i32::try_from(after.as_secs()).unwrap_or(0))
                .send()
                .await
                .map_err(|error| sqs_error("release", &error))?;
            Ok(())
        })
    }

    fn ack(&self, delivery: WakeDelivery) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(async move {
            let WakeOrigin::Queue { receipt, .. } = delivery.origin else {
                return Ok(());
            };
            self.client
                .delete_message()
                .queue_url(&self.queue_url)
                .receipt_handle(receipt)
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
                    aex_session_dynamodb::attr::s(format!("{}#", due_before.to_wire())),
                )
                .limit(i32::try_from(max).unwrap_or(50))
                .send()
                .await
                .map_err(|error| StoreError::Transport {
                    reason: format!("due_scan: {error}"),
                    retryable: true,
                })?;
            let mut wakes = Vec::new();
            for item in output.items() {
                if let Some(wake) = decode_due_entry(item)? {
                    wakes.push(wake);
                }
            }
            Ok(wakes)
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
        origin: WakeOrigin::Queue {
            receipt,
            receive_count,
        },
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
    let payload = value
        .get("payload")
        .ok_or_else(|| undecodable("wake payload", "`payload` is absent"))?;
    let session = parse_session(json_string(payload.get("sessionId"), "sessionId")?)?;
    let agent = parse_agent(json_string(payload.get("agentId"), "agentId")?)?;
    let work_id = json_string(value.get("workId"), "workId")?;
    if work_id.is_empty() {
        return Err(undecodable("wake payload", "`workId` is empty"));
    }
    let workspace = parse_workspace(json_string(value.get("workspaceId"), "workspaceId")?)?;
    let due = parse_timestamp(json_string(value.get("dueAt"), "dueAt")?, "wake payload")?;
    let priority = json_priority(value.get("priority"))?;
    Ok(DurableWake {
        id: WakeId(wake_identity(work_id)),
        work_id: work_id.to_owned(),
        key: AgentKey::new(session, agent),
        dedup_key: work_id.to_owned(),
        // The reason is a scheduling hint. What the agent actually owes is a total function
        // of its folded state, so a wake that lies about the reason cannot make it act
        // wrongly; it only makes the diagnostic wrong.
        reason: ParkReason::AwaitingUserMessage,
        due: Some(due),
        priority,
        tenant: workspace.to_string(),
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

fn undecodable(location: &str, reason: impl Into<String>) -> StoreError {
    StoreError::Undecodable {
        location: location.to_owned(),
        reason: reason.into(),
    }
}

fn json_string<'a>(
    value: Option<&'a serde_json::Value>,
    what: &str,
) -> Result<&'a str, StoreError> {
    value.and_then(serde_json::Value::as_str).ok_or_else(|| {
        undecodable(
            "wake payload",
            format!("`{what}` is absent or not a string"),
        )
    })
}

fn json_priority(value: Option<&serde_json::Value>) -> Result<u8, StoreError> {
    let parsed = match value {
        Some(serde_json::Value::Number(number)) => number.as_u64(),
        // DynamoDB Streams represents an `N` attribute as a JSON string. Pipes
        // preserves that representation when the input template extracts `.N`.
        Some(serde_json::Value::String(text)) => text.parse::<u64>().ok(),
        _ => None,
    };
    parsed
        .and_then(|number| u8::try_from(number).ok())
        .ok_or_else(|| undecodable("wake payload", "`priority` is not an unsigned byte"))
}

fn parse_session(text: &str) -> Result<SessionId, StoreError> {
    let value: aex_wire::ids::SessionId = text
        .parse()
        .map_err(|_| undecodable("wake payload", "`sessionId` is not a session identifier"))?;
    Ok(crate::translate::session_from_wire(value))
}

fn parse_agent(text: &str) -> Result<AgentId, StoreError> {
    let value: aex_wire::ids::AgentId = text
        .parse()
        .map_err(|_| undecodable("wake payload", "`agentId` is not an agent identifier"))?;
    Ok(crate::translate::agent_from_wire(value))
}

fn parse_workspace(text: &str) -> Result<aex_wire::ids::WorkspaceId, StoreError> {
    text.parse().map_err(|_| {
        undecodable(
            "wake payload",
            "`workspaceId` is not a workspace identifier",
        )
    })
}

fn parse_timestamp(text: &str, location: &str) -> Result<Timestamp, StoreError> {
    aex_wire::types::Timestamp::parse(text)
        .map(crate::translate::from_wire)
        .map_err(|error| undecodable(location, format!("`dueAt` is malformed: {error}")))
}

fn due_string<'a>(
    item: &'a aex_session_dynamodb::attr::Item,
    name: &str,
) -> Result<&'a str, StoreError> {
    item.get(name)
        .ok_or_else(|| undecodable("regional-work/gsi_due", format!("`{name}` is absent")))?
        .as_s()
        .map(String::as_str)
        .map_err(|_| undecodable("regional-work/gsi_due", format!("`{name}` is not a string")))
}

fn due_priority(item: &aex_session_dynamodb::attr::Item) -> Result<u8, StoreError> {
    item.get("priority")
        .ok_or_else(|| undecodable("regional-work/gsi_due", "`priority` is absent"))?
        .as_n()
        .ok()
        .and_then(|text| text.parse::<u64>().ok())
        .and_then(|number| u8::try_from(number).ok())
        .ok_or_else(|| {
            undecodable(
                "regional-work/gsi_due",
                "`priority` is not a DynamoDB unsigned byte",
            )
        })
}

fn decode_due_entry(
    item: &aex_session_dynamodb::attr::Item,
) -> Result<Option<DurableWake>, StoreError> {
    if due_string(item, "kind")? != "agent.wake" {
        return Ok(None);
    }
    if due_string(item, "state")? != "pending" {
        return Ok(None);
    }
    let work_id = due_string(item, "workId")?;
    let session = parse_session(due_string(item, "sessionId")?)?;
    let agent = parse_agent(due_string(item, "agentId")?)?;
    let workspace = parse_workspace(due_string(item, "workspaceId")?)?;
    let due = parse_timestamp(due_string(item, "dueAt")?, "regional-work/gsi_due")?;
    let priority = due_priority(item)?;
    Ok(Some(DurableWake {
        id: WakeId(wake_identity(work_id)),
        work_id: work_id.to_owned(),
        key: AgentKey::new(session, agent),
        dedup_key: work_id.to_owned(),
        reason: ParkReason::AwaitingUserMessage,
        due: Some(due),
        priority,
        tenant: workspace.to_string(),
    }))
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
    use super::{decode_body, decode_due_entry, decode_message};
    use aex_session_dynamodb::attr::{Item, n, s, stamp};
    use aex_wire::ids::{PrefixedId, Uuid7};

    fn body() -> String {
        let session =
            aex_wire::ids::SessionId::from_uuid7(Uuid7::compose(1_767_225_600_000, [1; 10]));
        let agent = aex_wire::ids::AgentId::from_uuid7(Uuid7::compose(1_767_225_600_001, [2; 10]));
        let workspace =
            aex_wire::ids::WorkspaceId::from_uuid7(Uuid7::compose(1_767_225_600_002, [3; 10]));
        format!(
            r#"{{"workId":"wrk_1","priority":2,"dueAt":"2026-01-01T00:00:00.000Z","workspaceId":"{workspace}","payload":{{"sessionId":"{session}","agentId":"{agent}"}}}}"#
        )
    }

    #[test]
    fn a_projected_row_decodes_into_the_agent_it_names() {
        let body = body();
        let expected: serde_json::Value = serde_json::from_str(&body).expect("fixture JSON");
        let wake = decode_body(&body).expect("a well-formed projection");
        assert_eq!(wake.work_id, "wrk_1");
        assert_eq!(wake.dedup_key, "wrk_1");
        assert_eq!(wake.priority, 2);
        assert_eq!(
            wake.tenant,
            expected["workspaceId"].as_str().expect("workspace string")
        );
        assert_eq!(
            wake.due,
            Some(aex_brain_domain::ids::Timestamp::from_millis(
                1_767_225_600_000
            ))
        );
    }

    #[test]
    fn a_dynamodb_number_extracted_by_pipes_keeps_its_priority() {
        let body = body().replace(r#""priority":2"#, r#""priority":"2""#);
        let wake = decode_body(&body).expect("DynamoDB N is represented as a JSON string");
        assert_eq!(wake.priority, 2);
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
        assert_eq!(delivery.receive_count(), Some(7));
    }

    fn due_entry() -> (Item, aex_brain_domain::ids::AgentKey, String) {
        let session =
            aex_wire::ids::SessionId::from_uuid7(Uuid7::compose(1_767_225_600_000, [1; 10]));
        let agent = aex_wire::ids::AgentId::from_uuid7(Uuid7::compose(1_767_225_600_001, [2; 10]));
        let workspace =
            aex_wire::ids::WorkspaceId::from_uuid7(Uuid7::compose(1_767_225_600_002, [3; 10]));
        let due =
            aex_wire::types::Timestamp::from_unix_millis(1_767_225_600_000).expect("in range");
        let item = [
            ("workId".to_owned(), s("wrk_due")),
            ("kind".to_owned(), s("agent.wake")),
            ("state".to_owned(), s("pending")),
            ("workspaceId".to_owned(), s(workspace.to_string())),
            ("sessionId".to_owned(), s(session.to_string())),
            ("agentId".to_owned(), s(agent.to_string())),
            ("dueAt".to_owned(), stamp(due)),
            ("priority".to_owned(), n(3)),
        ]
        .into_iter()
        .collect();
        (
            item,
            aex_brain_domain::ids::AgentKey::new(
                crate::translate::session_from_wire(session),
                crate::translate::agent_from_wire(agent),
            ),
            workspace.to_string(),
        )
    }

    #[test]
    fn a_due_index_row_keeps_its_real_agent_tenant_due_time_and_priority() {
        let (item, key, workspace) = due_entry();
        let wake = decode_due_entry(&item)
            .expect("the row decodes")
            .expect("it is an agent wake");
        assert_eq!(wake.key, key);
        assert_eq!(wake.tenant, workspace);
        assert_eq!(wake.priority, 3);
        assert_eq!(
            wake.due,
            Some(aex_brain_domain::ids::Timestamp::from_millis(
                1_767_225_600_000
            ))
        );
    }

    #[test]
    fn a_due_agent_wake_missing_its_agent_is_refused() {
        let (mut item, _, _) = due_entry();
        item.remove("agentId");
        assert!(decode_due_entry(&item).is_err());
    }

    #[test]
    fn a_terminal_row_left_in_the_sparse_index_is_never_resurrected() {
        let (mut item, _, _) = due_entry();
        item.insert("state".to_owned(), s("done"));
        assert_eq!(
            decode_due_entry(&item).expect("a terminal state is understood"),
            None
        );
    }
}
