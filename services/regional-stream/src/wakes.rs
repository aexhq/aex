//! Shared `DynamoDB` Streams readers used only as keyed latency hints.
//!
//! Each shard is read once per task. Records are `KEYS_ONLY` and only interrupt
//! an adaptive authority-read timer; response bytes always come from a fresh
//! table range read. Iterator expiry, throttling, resharding, duplicates and
//! process restarts therefore affect latency but cannot create or omit a frame.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aex_observation_domain::keys::ScopeKey;
use aex_wire::ids::{SessionId, WorkspaceId};
use aws_sdk_dynamodb::types::AttributeValue as TableAttribute;
// The bare `Display` of an SDK error is the outermost frame only, which reads as
// "service error" and names neither the operation nor the cause. Every one of
// these sites is the only record that a wake reader is failing.
use aws_sdk_dynamodbstreams::error::DisplayErrorContext;
use aws_sdk_dynamodbstreams::types::{
    AttributeValue, Record, Shard, ShardIteratorType, StreamStatus, StreamViewType,
};
use regional_observation_api::wake::WakeHub;

const DISCOVERY_INTERVAL: Duration = Duration::from_mins(1);
const EMPTY_SHARD_POLL: Duration = Duration::from_secs(1);
const RETRY_MIN: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(30);
const SESSION_CACHE_ENTRIES: usize = 4_096;

/// Which authority emitted one `KEYS_ONLY` record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthorityStream {
    /// `session-authority`, where only native `EVT#` rows wake event streams.
    Session,
    /// `observation-authority`, filtered by its published `OBS#` parser.
    Observation,
}

/// One validated stream to supervise.
#[derive(Clone, Debug)]
pub struct StreamSpec {
    /// Stream ARN.
    pub arn: String,
    /// Owning authority.
    pub authority: AuthorityStream,
}

/// Startup or discovery failure.
#[derive(Debug, thiserror::Error)]
pub enum WakeError {
    /// Provider call failed without exposing a provider response body.
    #[error("{operation} failed for {authority:?}: {reason}")]
    Provider {
        /// Which low-level call.
        operation: &'static str,
        /// Which authority.
        authority: AuthorityStream,
        /// Sanitized SDK error.
        reason: String,
    },
    /// Stream description was absent or contradicted the `KEYS_ONLY` contract.
    #[error("{authority:?} stream contract is invalid: {reason}")]
    Contract {
        /// Which authority.
        authority: AuthorityStream,
        /// Why startup was refused.
        reason: &'static str,
    },
}

/// Keeps the two stream supervisors alive for the process lifetime.
#[must_use]
pub struct WakeReaders {
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for WakeReaders {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Validates both configured streams and starts shard discovery/read loops.
///
/// # Errors
///
/// Fails closed before listener bind if either stream is not enabled,
/// `KEYS_ONLY`, describable, or has no stream description.
pub async fn start(
    streams: aws_sdk_dynamodbstreams::Client,
    table: aws_sdk_dynamodb::Client,
    session_table: String,
    specs: Vec<StreamSpec>,
    hub: WakeHub,
) -> Result<WakeReaders, WakeError> {
    let resolver = WorkspaceResolver::new(table, session_table);
    let mut tasks = Vec::with_capacity(specs.len());
    for spec in specs {
        let shards = describe(&streams, &spec).await?;
        tasks.push(tokio::spawn(supervise(
            streams.clone(),
            spec,
            hub.clone(),
            resolver.clone(),
            shards,
        )));
    }
    Ok(WakeReaders { tasks })
}

async fn describe(
    client: &aws_sdk_dynamodbstreams::Client,
    spec: &StreamSpec,
) -> Result<Vec<Shard>, WakeError> {
    let mut after: Option<String> = None;
    let mut shards = Vec::new();
    loop {
        let mut request = client.describe_stream().stream_arn(&spec.arn);
        if let Some(after) = after.as_deref() {
            request = request.exclusive_start_shard_id(after);
        }
        let output = request.send().await.map_err(|error| WakeError::Provider {
            operation: "DescribeStream",
            authority: spec.authority,
            reason: error.to_string(),
        })?;
        let description = output.stream_description().ok_or(WakeError::Contract {
            authority: spec.authority,
            reason: "the provider omitted StreamDescription",
        })?;
        if description.stream_status() != Some(&StreamStatus::Enabled) {
            return Err(WakeError::Contract {
                authority: spec.authority,
                reason: "the stream is not enabled",
            });
        }
        if description.stream_view_type() != Some(&StreamViewType::KeysOnly) {
            return Err(WakeError::Contract {
                authority: spec.authority,
                reason: "StreamViewType must be KEYS_ONLY",
            });
        }
        shards.extend_from_slice(description.shards());
        after = description.last_evaluated_shard_id().map(str::to_owned);
        if after.is_none() {
            break;
        }
    }
    Ok(shards)
}

async fn supervise(
    client: aws_sdk_dynamodbstreams::Client,
    spec: StreamSpec,
    hub: WakeHub,
    resolver: WorkspaceResolver,
    initial: Vec<Shard>,
) {
    let mut known = BTreeSet::new();
    spawn_new_shards(&client, &spec, &hub, &resolver, &mut known, initial);
    loop {
        tokio::time::sleep(DISCOVERY_INTERVAL).await;
        match describe(&client, &spec).await {
            Ok(shards) => spawn_new_shards(&client, &spec, &hub, &resolver, &mut known, shards),
            Err(error) => eprintln!("regional-stream: wake discovery degraded: {error}"),
        }
    }
}

fn spawn_new_shards(
    client: &aws_sdk_dynamodbstreams::Client,
    spec: &StreamSpec,
    hub: &WakeHub,
    resolver: &WorkspaceResolver,
    known: &mut BTreeSet<String>,
    shards: Vec<Shard>,
) {
    for shard in shards {
        let Some(shard_id) = shard.shard_id().map(str::to_owned) else {
            continue;
        };
        if !known.insert(shard_id.clone()) {
            continue;
        }
        tokio::spawn(read_shard(
            client.clone(),
            spec.clone(),
            shard_id,
            hub.clone(),
            resolver.clone(),
        ));
    }
}

async fn read_shard(
    client: aws_sdk_dynamodbstreams::Client,
    spec: StreamSpec,
    shard_id: String,
    hub: WakeHub,
    resolver: WorkspaceResolver,
) {
    let mut retry = RETRY_MIN;
    loop {
        let iterator = client
            .get_shard_iterator()
            .stream_arn(&spec.arn)
            .shard_id(&shard_id)
            .shard_iterator_type(ShardIteratorType::Latest)
            .send()
            .await;
        let Ok(output) = iterator else {
            eprintln!(
                "regional-stream: cannot acquire {:?} shard iterator: {}",
                spec.authority,
                DisplayErrorContext(&iterator.expect_err("the result is known to be an error"))
            );
            tokio::time::sleep(retry).await;
            retry = retry.saturating_mul(2).min(RETRY_MAX);
            continue;
        };
        let Some(mut iterator) = output.shard_iterator().map(str::to_owned) else {
            eprintln!(
                "regional-stream: {:?} omitted a shard iterator",
                spec.authority
            );
            tokio::time::sleep(retry).await;
            retry = retry.saturating_mul(2).min(RETRY_MAX);
            continue;
        };
        retry = RETRY_MIN;
        loop {
            let result = client
                .get_records()
                .shard_iterator(iterator)
                .limit(1_000)
                .send()
                .await;
            let Ok(output) = result else {
                eprintln!(
                    "regional-stream: {:?} wake read degraded: {}",
                    spec.authority,
                    DisplayErrorContext(&result.expect_err("the result is known to be an error"))
                );
                break;
            };
            for record in output.records() {
                if let Some(scope) = classify(record, spec.authority) {
                    resolver.notify(&hub, scope).await;
                }
            }
            let count = output.records().len();
            let Some(next) = output.next_shard_iterator().map(str::to_owned) else {
                return;
            };
            iterator = next;
            if count < 1_000 {
                tokio::time::sleep(EMPTY_SHARD_POLL).await;
            }
        }
        tokio::time::sleep(retry).await;
        retry = retry.saturating_mul(2).min(RETRY_MAX);
    }
}

fn classify(record: &Record, authority: AuthorityStream) -> Option<ScopeKey> {
    let stream = record.dynamodb()?;
    if stream.stream_view_type() != Some(&StreamViewType::KeysOnly) {
        return None;
    }
    let keys = stream.keys()?;
    let pk = string(keys.get("pk")?)?;
    let sk = string(keys.get("sk")?)?;
    match authority {
        AuthorityStream::Observation => {
            aex_observation_store_aws::keys::parse_observation_pk(pk).map(|wake| wake.scope)
        }
        AuthorityStream::Session => {
            aex_session_dynamodb::stream_keys::parse_event(pk, sk).map(ScopeKey::Session)
        }
    }
}

fn string(value: &AttributeValue) -> Option<&str> {
    match value {
        AttributeValue::S(value) => Some(value),
        _ => None,
    }
}

#[derive(Clone)]
struct WorkspaceResolver {
    table: aws_sdk_dynamodb::Client,
    table_name: Arc<str>,
    cache: Arc<Mutex<BTreeMap<SessionId, WorkspaceId>>>,
}

impl WorkspaceResolver {
    fn new(table: aws_sdk_dynamodb::Client, table_name: String) -> Self {
        Self {
            table,
            table_name: table_name.into(),
            cache: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    async fn notify(&self, hub: &WakeHub, scope: ScopeKey) {
        hub.notify(scope);
        let ScopeKey::Session(session) = scope else {
            return;
        };
        if let Some(workspace) = self.workspace(session).await {
            hub.notify(ScopeKey::Workspace(workspace));
        }
    }

    async fn workspace(&self, session: SessionId) -> Option<WorkspaceId> {
        if let Some(workspace) = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&session)
            .copied()
        {
            return Some(workspace);
        }
        let (pk, sk) = aex_session_dynamodb::stream_keys::head(session);
        let item = self
            .table
            .get_item()
            .table_name(self.table_name.as_ref())
            .key("pk", TableAttribute::S(pk))
            .key("sk", TableAttribute::S(sk.to_owned()))
            .consistent_read(true)
            .send()
            .await
            .ok()?
            .item?;
        let workspace = aex_session_dynamodb::stream_keys::head_workspace(&item)?;
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cache.len() >= SESSION_CACHE_ENTRIES
            && let Some(oldest) = cache.keys().next().copied()
        {
            cache.remove(&oldest);
        }
        cache.insert(session, workspace);
        Some(workspace)
    }
}

#[cfg(test)]
mod tests {
    use aex_observation_domain::keys::{BucketHour, ScopeKey, observation_pk};
    use aex_observation_domain::signal::Signal;
    use aex_wire::ids::{PrefixedId as _, SessionId, Uuid7};
    use aws_sdk_dynamodbstreams::types::{AttributeValue, Record, StreamRecord, StreamViewType};

    use super::{AuthorityStream, classify};

    fn session(seed: u8) -> SessionId {
        SessionId::from_uuid7(Uuid7::compose(1, [seed; 10]))
    }

    fn record(pk: String, sk: &str, view: StreamViewType) -> Record {
        Record::builder()
            .dynamodb(
                StreamRecord::builder()
                    .keys("pk", AttributeValue::S(pk))
                    .keys("sk", AttributeValue::S(sk.to_owned()))
                    .stream_view_type(view)
                    .build(),
            )
            .build()
    }

    #[test]
    fn observation_and_event_filters_consume_keys_only() {
        let scope = ScopeKey::Session(session(1));
        let bucket = BucketHour::parse("2026-08-02T12").expect("bucket");
        let observation = record(
            observation_pk(&scope, Signal::Logs, bucket, 0),
            "00000000000000000001",
            StreamViewType::KeysOnly,
        );
        assert_eq!(
            classify(&observation, AuthorityStream::Observation),
            Some(scope)
        );
        let event = record(
            format!("SESSION#{}", session(2)),
            "EVT#00000000000000000001",
            StreamViewType::KeysOnly,
        );
        assert_eq!(
            classify(&event, AuthorityStream::Session),
            Some(ScopeKey::Session(session(2)))
        );
        assert_eq!(
            classify(
                &record(
                    format!("SESSION#{}", session(2)),
                    "HEAD",
                    StreamViewType::KeysOnly
                ),
                AuthorityStream::Session
            ),
            None
        );
        assert_eq!(
            classify(
                &record("OBS#not-a-key".to_owned(), "1", StreamViewType::NewImage),
                AuthorityStream::Observation
            ),
            None,
            "a payload-bearing stream is never consumed"
        );
    }
}
