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
    BoxFuture, DueRowIsolation, DueRowIsolationReason, DueScanCursor, DueScanPage, DurableWake,
    MAX_DUE_ROW_ISOLATIONS, MalformedWakeDelivery, MalformedWakeReason, StoreError, WakeBatch,
    WakeDelivery, WakeOrigin, WakeQueue, WakeState,
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

/// The exact `SQS` action set the Brain task role needs on the wake queue.
///
/// Every entry is an operation [`SqsWakeQueue`] actually invokes, and the test below holds
/// the list and the call sites to the same set, so a queue call added without a grant fails
/// here rather than in a plane. `sqs:GetQueueAttributes` is in it because of
/// [`SqsWakeQueue::probe`]: readiness is a real signed request, so a role allowed to drain
/// the queue but not to describe it reports a perfectly healthy queue as unreachable for as
/// long as the task lives.
///
/// Actions only. The queue ARN is a plane-specific value the deployment root that creates
/// the queue owns; a crate that guessed one would scope the grant to the wrong queue.
pub const WAKE_QUEUE_IAM_ACTIONS: [&str; 4] = [
    "sqs:ChangeMessageVisibility",
    "sqs:DeleteMessage",
    "sqs:GetQueueAttributes",
    "sqs:ReceiveMessage",
];

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

    /// Proves the configured queue exists and this task may address it.
    ///
    /// `GetQueueAttributes` rather than `ReceiveMessage`: a probe that received would take a
    /// delivery out of the queue, start its visibility timeout and race the pump for the very
    /// work it is meant to be reporting on. Reading one attribute costs a signed round trip
    /// and moves nothing.
    ///
    /// Requires `sqs:GetQueueAttributes` on the Brain task role, declared with the rest of
    /// this adapter's queue grant in [`WAKE_QUEUE_IAM_ACTIONS`]. Attaching that grant is an
    /// infrastructure change this crate cannot make; without it a task whose queue is
    /// perfectly reachable stays unready, and the refusal names the operation.
    ///
    /// # Errors
    ///
    /// [`StoreError::Transport`] when the queue does not answer or the role may not ask.
    pub async fn probe(&self) -> Result<(), StoreError> {
        self.client
            .get_queue_attributes()
            .queue_url(&self.queue_url)
            .attribute_names(aws_sdk_sqs::types::QueueAttributeName::QueueArn)
            .send()
            .await
            .map_err(|error| sqs_error("probe", &error))?;
        Ok(())
    }
}

impl WakeQueue for SqsWakeQueue {
    fn receive(
        &self,
        max: usize,
        wait: core::time::Duration,
    ) -> BoxFuture<'_, Result<WakeBatch, StoreError>> {
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
            Ok(decode_messages(output.messages.unwrap_or_default()))
        })
    }

    fn release_malformed(
        &self,
        delivery: MalformedWakeDelivery,
        after: core::time::Duration,
    ) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(async move {
            let receipt = delivery.receipt.ok_or_else(|| {
                undecodable(
                    "wake delivery",
                    "a malformed message carried no receipt handle and cannot be released",
                )
            })?;
            self.client
                .change_message_visibility()
                .queue_url(&self.queue_url)
                .receipt_handle(receipt)
                .visibility_timeout(i32::try_from(after.as_secs()).unwrap_or(0))
                .send()
                .await
                .map_err(|error| sqs_error("release_malformed", &error))?;
            Ok(())
        })
    }

    fn ack_malformed(
        &self,
        delivery: MalformedWakeDelivery,
    ) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(async move {
            let receipt = delivery.receipt.ok_or_else(|| {
                undecodable(
                    "wake delivery",
                    "a malformed message carried no receipt handle and cannot be acknowledged",
                )
            })?;
            self.client
                .delete_message()
                .queue_url(&self.queue_url)
                .receipt_handle(receipt)
                .send()
                .await
                .map_err(|error| sqs_error("ack_malformed", &error))?;
            Ok(())
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
        after: Option<DueScanCursor>,
    ) -> BoxFuture<'_, Result<DueScanPage, StoreError>> {
        Box::pin(async move {
            let due_before =
                aex_wire::types::Timestamp::from_unix_millis(now.millis()).map_err(|_| {
                    StoreError::Undecodable {
                        location: "due scan".to_owned(),
                        reason: "the scan instant is outside the wire range".to_owned(),
                    }
                })?;
            let exclusive_start_key = after.as_ref().map(cursor_key).transpose()?;
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
                    aex_session_dynamodb::attr::s(format!("{}#\u{10ffff}", due_before.to_wire())),
                )
                .limit(i32::try_from(max).unwrap_or(50))
                .set_exclusive_start_key(exclusive_start_key)
                .send()
                .await
                .map_err(|error| StoreError::Transport {
                    reason: format!("due_scan: {error}"),
                    retryable: true,
                })?;
            let next = output
                .last_evaluated_key()
                .map(cursor_from_key)
                .transpose()?;
            let decoded = decode_due_entries(output.items());
            Ok(DueScanPage {
                wakes: decoded.wakes,
                next,
                malformed: decoded.malformed,
                isolations: decoded.isolations,
            })
        })
    }
}

fn decode_messages(messages: Vec<aws_sdk_sqs::types::Message>) -> WakeBatch {
    let mut batch = WakeBatch::default();
    for message in messages {
        let malformed = malformed_delivery(&message);
        match decode_message(message) {
            Ok(delivery) => batch.deliveries.push(delivery),
            Err(_) => batch.malformed.push(malformed),
        }
    }
    batch
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

fn malformed_delivery(message: &aws_sdk_sqs::types::Message) -> MalformedWakeDelivery {
    let reason = if message.receipt_handle.is_none() {
        MalformedWakeReason::MissingReceipt
    } else if message.body.is_none() {
        MalformedWakeReason::MissingBody
    } else {
        MalformedWakeReason::InvalidProjection
    };
    let receive_count = message
        .attributes
        .as_ref()
        .and_then(|it| it.get(&MessageSystemAttributeName::ApproximateReceiveCount))
        .and_then(|it| it.parse::<u32>().ok())
        .unwrap_or(1);
    let digest = blake3::hash(
        message
            .body
            .as_deref()
            .unwrap_or("<missing-body>")
            .as_bytes(),
    );
    MalformedWakeDelivery {
        receipt: message.receipt_handle.clone(),
        receive_count,
        reason,
        fingerprint: digest.to_hex()[..16].to_owned(),
    }
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

const DUE_CURSOR_PARTS: [&str; 4] = [
    aex_session_dynamodb::attr::PK,
    aex_session_dynamodb::attr::SK,
    aex_work_dynamodb::keys::DUE_PK,
    aex_work_dynamodb::keys::DUE_SK,
];

fn cursor_key(cursor: &DueScanCursor) -> Result<aex_session_dynamodb::attr::Item, StoreError> {
    DUE_CURSOR_PARTS
        .into_iter()
        .map(|name| {
            cursor
                .parts()
                .get(name)
                .cloned()
                .map(|value| (name.to_owned(), aex_session_dynamodb::attr::s(value)))
                .ok_or_else(|| {
                    undecodable(
                        "regional-work/gsi_due cursor",
                        format!("`{name}` is absent"),
                    )
                })
        })
        .collect()
}

fn cursor_from_key(key: &aex_session_dynamodb::attr::Item) -> Result<DueScanCursor, StoreError> {
    let parts = DUE_CURSOR_PARTS
        .into_iter()
        .map(|name| due_string(key, name).map(|value| (name, value.to_owned())))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DueScanCursor::new(parts))
}

#[derive(Debug, PartialEq, Eq)]
struct DecodedDueEntries {
    wakes: Vec<DurableWake>,
    malformed: usize,
    isolations: Vec<DueRowIsolation>,
}

fn decode_due_entries<'a>(
    items: impl IntoIterator<Item = &'a aex_session_dynamodb::attr::Item>,
) -> DecodedDueEntries {
    let mut wakes = Vec::new();
    let mut malformed = 0;
    let mut isolations = Vec::new();
    for item in items {
        match decode_due_entry(item) {
            Ok(Some(wake)) => wakes.push(wake),
            Ok(None) => {}
            Err(reason) => {
                malformed += 1;
                if isolations.len() < MAX_DUE_ROW_ISOLATIONS {
                    isolations.push(DueRowIsolation {
                        reason,
                        fingerprint: due_row_fingerprint(item),
                    });
                }
            }
        }
    }
    DecodedDueEntries {
        wakes,
        malformed,
        isolations,
    }
}

fn decode_due_entry(
    item: &aex_session_dynamodb::attr::Item,
) -> Result<Option<DurableWake>, DueRowIsolationReason> {
    let projected_string =
        |name| due_string(item, name).map_err(|_| DueRowIsolationReason::MalformedProjection);
    if projected_string("kind")? != "agent.wake" {
        return Ok(None);
    }
    let work_id = projected_string("workId")?;
    let due_wire = aex_wire::types::Timestamp::parse(projected_string("dueAt")?)
        .map_err(|_| DueRowIsolationReason::MalformedProjection)?;
    let priority = due_priority(item).map_err(|_| DueRowIsolationReason::MalformedProjection)?;
    validate_due_keys(item, work_id, due_wire, priority)?;

    let state = projected_string("state")?;
    if state != "pending" {
        if !aex_work_dynamodb::keys::STATES.contains(&state) {
            return Err(DueRowIsolationReason::MalformedProjection);
        }
        return Ok(None);
    }
    let session = parse_session(projected_string("sessionId")?)
        .map_err(|_| DueRowIsolationReason::MalformedProjection)?;
    let agent = parse_agent(projected_string("agentId")?)
        .map_err(|_| DueRowIsolationReason::MalformedProjection)?;
    let workspace = parse_workspace(projected_string("workspaceId")?)
        .map_err(|_| DueRowIsolationReason::MalformedProjection)?;
    let due = crate::translate::from_wire(due_wire);
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

fn validate_due_keys(
    item: &aex_session_dynamodb::attr::Item,
    work_id: &str,
    due: aex_wire::types::Timestamp,
    priority: u8,
) -> Result<(), DueRowIsolationReason> {
    let expected_base = aex_work_dynamodb::keys::work(work_id)
        .map_err(|_| DueRowIsolationReason::MalformedProjection)?;
    if due_string(item, aex_session_dynamodb::attr::PK)
        .map_err(|_| DueRowIsolationReason::MalformedProjection)?
        != expected_base.pk
        || due_string(item, aex_session_dynamodb::attr::SK)
            .map_err(|_| DueRowIsolationReason::MalformedProjection)?
            != expected_base.sk
    {
        return Err(DueRowIsolationReason::BaseKeyMismatch);
    }

    if due_string(item, aex_work_dynamodb::keys::DUE_PK)
        .map_err(|_| DueRowIsolationReason::MalformedProjection)?
        != aex_work_dynamodb::keys::due_partition(work_id)
    {
        return Err(DueRowIsolationReason::ShardMismatch);
    }

    let effective = aex_work_dynamodb::keys::effective_due_at(due, priority)
        .map_err(|_| DueRowIsolationReason::MalformedProjection)?;
    let expected_position = aex_work_dynamodb::keys::due_sort(effective, work_id)
        .map_err(|_| DueRowIsolationReason::MalformedProjection)?;
    if due_string(item, aex_work_dynamodb::keys::DUE_SK)
        .map_err(|_| DueRowIsolationReason::MalformedProjection)?
        != expected_position
    {
        return Err(DueRowIsolationReason::DuePositionMismatch);
    }
    Ok(())
}

fn due_row_fingerprint(item: &aex_session_dynamodb::attr::Item) -> String {
    let mut hasher = blake3::Hasher::new();
    for name in DUE_CURSOR_PARTS {
        hasher.update(name.as_bytes());
        hasher.update(&[0]);
        if let Some(value) = item.get(name).and_then(|value| value.as_s().ok()) {
            hasher.update(value.as_bytes());
        } else {
            hasher.update(b"<absent-or-not-string>");
        }
        hasher.update(&[0xff]);
    }
    hasher.finalize().to_hex()[..16].to_owned()
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
    use super::{
        WAKE_QUEUE_IAM_ACTIONS, cursor_from_key, cursor_key, decode_body, decode_due_entries,
        decode_due_entry, decode_message, decode_messages,
    };
    use aex_brain_application::ports::{DueRowIsolationReason, MAX_DUE_ROW_ISOLATIONS};
    use aex_session_dynamodb::attr::{Item, n, s, stamp};
    use aex_wire::ids::{PrefixedId, Uuid7};
    use std::collections::BTreeSet;

    /// Every `SQS` operation this adapter invokes, read out of its own source.
    ///
    /// Comments are dropped and whitespace removed first, so the multi-line builder style
    /// the call sites are formatted in still reduces to one receiver token. The due-scan
    /// reads go through `self.due.client`, which is deliberately a different token: they are
    /// `DynamoDB` calls and belong to the table grant, not this one.
    fn called_queue_operations() -> BTreeSet<String> {
        // Assembled rather than written out, because the scan reads this very file and a
        // verbatim receiver literal would match its own definition.
        let receiver = format!("self.{}.", "client");
        let dense: String = include_str!("wake.rs")
            .lines()
            .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
            .flat_map(str::chars)
            .filter(|character| !character.is_whitespace())
            .collect();
        let mut found = BTreeSet::new();
        let mut rest = dense.as_str();
        while let Some(index) = rest.find(receiver.as_str()) {
            rest = &rest[index + receiver.len()..];
            let end = rest.find('(').expect("a queue client access is a call");
            let operation = &rest[..end];
            assert!(
                operation
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_'),
                "`{operation}` is not a queue operation; the grant scan cannot classify it"
            );
            found.insert(operation.to_owned());
        }
        found
    }

    /// `sqs:GetQueueAttributes` becomes `get_queue_attributes`.
    fn operation_of(action: &str) -> String {
        let mut operation = String::new();
        for character in action
            .strip_prefix("sqs:")
            .expect("every declared action is an SQS action")
            .chars()
        {
            if character.is_ascii_uppercase() && !operation.is_empty() {
                operation.push('_');
            }
            operation.push(character.to_ascii_lowercase());
        }
        operation
    }

    /// The declaration is a fact about the code rather than a comment beside it. A queue
    /// call added without a grant fails here, not in a plane whose only symptom is a task
    /// that never becomes ready.
    #[test]
    fn the_declared_queue_grant_is_exactly_the_set_of_calls_this_adapter_makes() {
        let declared: BTreeSet<String> = WAKE_QUEUE_IAM_ACTIONS
            .iter()
            .copied()
            .map(operation_of)
            .collect();
        assert_eq!(
            declared.len(),
            WAKE_QUEUE_IAM_ACTIONS.len(),
            "the declared grant repeats an action"
        );
        assert_eq!(called_queue_operations(), declared);
    }

    /// The readiness probe costs a signed round trip against the real queue, so the action
    /// it needs is part of the grant rather than an assumption about it.
    #[test]
    fn the_probe_action_is_granted_and_no_entry_is_widened_to_a_wildcard() {
        assert!(
            WAKE_QUEUE_IAM_ACTIONS.contains(&"sqs:GetQueueAttributes"),
            "readiness probes the queue and cannot pass without the action it calls"
        );
        assert!(
            WAKE_QUEUE_IAM_ACTIONS
                .iter()
                .all(|action| action.starts_with("sqs:") && !action.contains('*')),
            "the wake-queue grant reaches one service and names every action"
        );
        assert!(
            WAKE_QUEUE_IAM_ACTIONS
                .windows(2)
                .all(|pair| pair[0] < pair[1]),
            "the grant is sorted so review sees an ordering, not a history"
        );
    }

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

    #[test]
    fn one_malformed_queue_record_does_not_reject_valid_siblings() {
        let valid = aws_sdk_sqs::types::Message::builder()
            .body(body())
            .receipt_handle("valid-rh")
            .build();
        let malformed = aws_sdk_sqs::types::Message::builder()
            .body("not-json")
            .receipt_handle("bad-rh")
            .attributes(
                aws_sdk_sqs::types::MessageSystemAttributeName::ApproximateReceiveCount,
                "4",
            )
            .build();

        let batch = decode_messages(vec![malformed, valid]);
        assert_eq!(batch.deliveries.len(), 1);
        assert_eq!(batch.malformed.len(), 1);
        assert_eq!(batch.malformed[0].receive_count, 4);
        assert_eq!(batch.malformed[0].fingerprint.len(), 16);
        assert_eq!(batch.malformed[0].receipt.as_deref(), Some("bad-rh"));
    }

    fn due_entry_for(
        work_id: &str,
        due_millis: i64,
    ) -> (Item, aex_brain_domain::ids::AgentKey, String) {
        let session =
            aex_wire::ids::SessionId::from_uuid7(Uuid7::compose(1_767_225_600_000, [1; 10]));
        let agent = aex_wire::ids::AgentId::from_uuid7(Uuid7::compose(1_767_225_600_001, [2; 10]));
        let workspace =
            aex_wire::ids::WorkspaceId::from_uuid7(Uuid7::compose(1_767_225_600_002, [3; 10]));
        let due = aex_wire::types::Timestamp::from_unix_millis(due_millis).expect("in range");
        let base = aex_work_dynamodb::keys::work(work_id).expect("a fixture work key");
        let effective =
            aex_work_dynamodb::keys::effective_due_at(due, 3).expect("priority three is valid");
        let item = [
            ("pk".to_owned(), s(base.pk)),
            ("sk".to_owned(), s(base.sk)),
            (
                "dueShardPk".to_owned(),
                s(aex_work_dynamodb::keys::due_partition(work_id)),
            ),
            (
                "dueShardSk".to_owned(),
                s(aex_work_dynamodb::keys::due_sort(effective, work_id).expect("a due key")),
            ),
            ("workId".to_owned(), s(work_id)),
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

    fn due_entry() -> (Item, aex_brain_domain::ids::AgentKey, String) {
        due_entry_for("wrk_due", 1_767_225_600_000)
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
    fn one_malformed_due_row_does_not_discard_its_valid_siblings() {
        let (first, _, _) = due_entry();
        let mut malformed = first.clone();
        malformed.remove("agentId");
        let (last, _, _) = due_entry_for("wrk_after_bad", 1_767_225_600_000);

        let decoded = decode_due_entries([&first, &malformed, &last]);
        assert_eq!(decoded.malformed, 1);
        assert_eq!(decoded.isolations.len(), 1);
        assert_eq!(
            decoded.isolations[0].reason,
            DueRowIsolationReason::MalformedProjection
        );
        assert_eq!(decoded.isolations[0].fingerprint.len(), 16);
        assert_eq!(decoded.wakes.len(), 2);
        assert_eq!(decoded.wakes[1].work_id, "wrk_after_bad");
    }

    #[test]
    fn a_due_row_must_match_both_parts_of_its_base_key() {
        for attribute in ["pk", "sk"] {
            let (mut item, _, _) = due_entry();
            item.insert(attribute.to_owned(), s("forged"));
            assert_eq!(
                decode_due_entry(&item),
                Err(DueRowIsolationReason::BaseKeyMismatch),
                "{attribute}"
            );
        }

        let (mut forged_work, _, _) = due_entry();
        forged_work.insert("workId".to_owned(), s("wrk_forged"));
        assert_eq!(
            decode_due_entry(&forged_work),
            Err(DueRowIsolationReason::BaseKeyMismatch)
        );
    }

    #[test]
    fn a_due_row_must_live_in_the_shard_derived_from_its_work_identity() {
        let (mut item, _, _) = due_entry();
        item.insert("dueShardPk".to_owned(), s("DUE#9999"));
        assert_eq!(
            decode_due_entry(&item),
            Err(DueRowIsolationReason::ShardMismatch)
        );
    }

    #[test]
    fn a_row_forged_into_an_earlier_position_cannot_wake_future_work() {
        let (mut item, _, _) = due_entry();
        let future = aex_wire::types::Timestamp::from_unix_millis(1_767_312_000_000)
            .expect("the future fixture is in range");
        item.insert("dueAt".to_owned(), stamp(future));
        assert_eq!(
            decode_due_entry(&item),
            Err(DueRowIsolationReason::DuePositionMismatch)
        );

        let (mut priority, _, _) = due_entry();
        priority.insert("priority".to_owned(), n(4));
        assert_eq!(
            decode_due_entry(&priority),
            Err(DueRowIsolationReason::DuePositionMismatch)
        );

        let (mut identity, _, _) = due_entry();
        identity.insert(
            "dueShardSk".to_owned(),
            s("2025-12-31T23:55:00.000Z#wrk_someone_else"),
        );
        assert_eq!(
            decode_due_entry(&identity),
            Err(DueRowIsolationReason::DuePositionMismatch)
        );
    }

    #[test]
    fn due_isolation_diagnostics_are_bounded_and_never_expose_row_values() {
        let rows: Vec<Item> = (0..MAX_DUE_ROW_ISOLATIONS + 3)
            .map(|index| {
                let (mut item, _, _) =
                    due_entry_for(&format!("wrk_bad_{index}"), 1_767_225_600_000);
                item.remove("agentId");
                item
            })
            .collect();
        let decoded = decode_due_entries(&rows);
        assert_eq!(decoded.malformed, MAX_DUE_ROW_ISOLATIONS + 3);
        assert_eq!(decoded.isolations.len(), MAX_DUE_ROW_ISOLATIONS);
        assert!(decoded.isolations.iter().all(|isolation| {
            isolation.fingerprint.len() == 16
                && isolation
                    .fingerprint
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
        }));
    }

    #[test]
    fn a_native_due_cursor_round_trips_every_base_and_index_key_part() {
        let key = [
            ("pk".to_owned(), s("WORK#wrk_10")),
            ("sk".to_owned(), s("STATE")),
            ("dueShardPk".to_owned(), s("DUE#0007")),
            (
                "dueShardSk".to_owned(),
                s("2026-01-01T00:00:00.000Z#wrk_10"),
            ),
        ]
        .into_iter()
        .collect();
        let cursor = cursor_from_key(&key).expect("the native key becomes an opaque cursor");
        assert_eq!(
            cursor_key(&cursor).expect("the opaque cursor becomes an exclusive start key"),
            key
        );
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
