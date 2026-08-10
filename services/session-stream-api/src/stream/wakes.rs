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
use regional_observation_api::counters::{ReadCounter, ReadCounters};
use regional_observation_api::wake::WakeHub;

const DISCOVERY_INTERVAL: Duration = Duration::from_mins(1);
const EMPTY_SHARD_POLL: Duration = Duration::from_secs(1);
const RETRY_MIN: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(30);
const SESSION_CACHE_ENTRIES: usize = 4_096;

/// How many session-to-workspace resolutions one record batch holds open.
///
/// The bound converts the shared HTTP pool's appetite, not a table quota — an
/// on-demand table has none. A serial resolver made a 1,000-record batch pay up
/// to a thousand sequential point reads before the next `GetRecords`, which is
/// latency the wake path exists to remove.
const WORKSPACE_RESOLUTION_CONCURRENCY: usize = 8;

/// Escalating retry delay for one shard reader.
///
/// Only a successful `GetRecords` proves the shard readable, so only success
/// restores the floor. Resetting after `GetShardIterator` — the shape this
/// replaced — pinned a persistent `GetRecords` failure at [`RETRY_MIN`]:
/// every cycle re-acquired an iterator, reset, failed and slept the minimum
/// again, so [`RETRY_MAX`] was unreachable on exactly the failure that
/// needed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Backoff {
    delay: Duration,
}

impl Backoff {
    const fn new() -> Self {
        Self { delay: RETRY_MIN }
    }

    /// The delay to sleep before the next attempt. Each failure doubles the
    /// following one, up to [`RETRY_MAX`].
    fn failure(&mut self) -> Duration {
        let delay = self.delay;
        self.delay = self.delay.saturating_mul(2).min(RETRY_MAX);
        delay
    }

    /// A successful `GetRecords` — an empty page included — proves the shard
    /// readable and restores the floor.
    fn success(&mut self) {
        self.delay = RETRY_MIN;
    }
}

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
    counters: Arc<ReadCounters>,
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
            Arc::clone(&counters),
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
    counters: Arc<ReadCounters>,
) {
    let mut known = BTreeSet::new();
    spawn_new_shards(
        &client, &spec, &hub, &resolver, &mut known, initial, &counters,
    );
    loop {
        tokio::time::sleep(DISCOVERY_INTERVAL).await;
        match describe(&client, &spec).await {
            Ok(shards) => {
                spawn_new_shards(
                    &client, &spec, &hub, &resolver, &mut known, shards, &counters,
                );
            }
            Err(error) => {
                counters.record(ReadCounter::WakeReaderDegraded);
                eprintln!("regional-stream: wake discovery degraded: {error}");
            }
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
    counters: &Arc<ReadCounters>,
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
            Arc::clone(counters),
        ));
    }
}

async fn read_shard(
    client: aws_sdk_dynamodbstreams::Client,
    spec: StreamSpec,
    shard_id: String,
    hub: WakeHub,
    resolver: WorkspaceResolver,
    counters: Arc<ReadCounters>,
) {
    let mut retry = Backoff::new();
    loop {
        let iterator = client
            .get_shard_iterator()
            .stream_arn(&spec.arn)
            .shard_id(&shard_id)
            .shard_iterator_type(ShardIteratorType::Latest)
            .send()
            .await;
        let Ok(output) = iterator else {
            counters.record(ReadCounter::WakeReaderDegraded);
            eprintln!(
                "regional-stream: cannot acquire {:?} shard iterator: {}",
                spec.authority,
                DisplayErrorContext(&iterator.expect_err("the result is known to be an error"))
            );
            tokio::time::sleep(retry.failure()).await;
            continue;
        };
        let Some(mut iterator) = output.shard_iterator().map(str::to_owned) else {
            counters.record(ReadCounter::WakeReaderDegraded);
            eprintln!(
                "regional-stream: {:?} omitted a shard iterator",
                spec.authority
            );
            tokio::time::sleep(retry.failure()).await;
            continue;
        };
        // The backoff is deliberately not reset here. An acquired iterator
        // proves nothing about reading records, and a reset on acquisition
        // pinned a persistent `GetRecords` failure at the floor for ever.
        loop {
            let result = client
                .get_records()
                .shard_iterator(iterator)
                .limit(1_000)
                .send()
                .await;
            let Ok(output) = result else {
                counters.record(ReadCounter::WakeReaderDegraded);
                eprintln!(
                    "regional-stream: {:?} wake read degraded: {}",
                    spec.authority,
                    DisplayErrorContext(&result.expect_err("the result is known to be an error"))
                );
                break;
            };
            retry.success();
            // An observation record's key carries its workspace, so both its
            // own scope and the workspace scope above it fire immediately and
            // with no table read at all. Only a session-authority `EVT#` row
            // still needs the session-to-workspace binding resolved, and those
            // are deduplicated per batch and driven concurrently rather than one
            // record at a time.
            let mut sessions = BTreeSet::new();
            for record in output.records() {
                match classify(record, spec.authority) {
                    Some(WakeTarget::Scope(scope)) => {
                        hub.notify(scope);
                        if let ScopeKey::Session { workspace, .. } = scope {
                            hub.notify(ScopeKey::Workspace(workspace));
                        }
                    }
                    Some(WakeTarget::Session(session)) => {
                        sessions.insert(session);
                    }
                    None => {}
                }
            }
            resolver.notify_sessions(&hub, sessions).await;
            let count = output.records().len();
            let Some(next) = output.next_shard_iterator().map(str::to_owned) else {
                return;
            };
            iterator = next;
            if count < 1_000 {
                tokio::time::sleep(EMPTY_SHARD_POLL).await;
            }
        }
        tokio::time::sleep(retry.failure()).await;
    }
}

/// What one classified record identifies.
///
/// The two authorities key differently. An `observation-authority` partition key
/// carries its workspace, so it yields a whole [`ScopeKey`] and needs nothing
/// else. A `session-authority` `EVT#` key carries only the session, so its
/// workspace has to be resolved from the immutable session head before any scope
/// can be named — the one place a wake still pays for a read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WakeTarget {
    /// A scope the record's own key fully determines.
    Scope(ScopeKey),
    /// A session whose owning workspace is not in the record.
    Session(SessionId),
}

fn classify(record: &Record, authority: AuthorityStream) -> Option<WakeTarget> {
    let stream = record.dynamodb()?;
    if stream.stream_view_type() != Some(&StreamViewType::KeysOnly) {
        return None;
    }
    let keys = stream.keys()?;
    let pk = string(keys.get("pk")?)?;
    let sk = string(keys.get("sk")?)?;
    match authority {
        AuthorityStream::Observation => {
            aex_observation_store_dynamodb::keys::parse_observation_pk(pk)
                .map(|wake| WakeTarget::Scope(wake.scope))
        }
        AuthorityStream::Session => {
            aex_session_dynamodb::stream_keys::parse_event(pk, sk).map(WakeTarget::Session)
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

    /// Wakes both scopes of each distinct session in one batch.
    ///
    /// The session scope needs the resolution too: a scope names its workspace,
    /// so a session event cannot be turned into a hint until the binding is
    /// known. An unresolvable session is skipped rather than hinted under a
    /// guessed owner — a lost hint costs one poll interval, a hint under the
    /// wrong key would wake a stranger's socket.
    ///
    /// The resolutions are driven at a bounded width and in no particular
    /// order: a wake is a keyed latency hint, never truth, so which session is
    /// notified first cannot matter.
    async fn notify_sessions(&self, hub: &WakeHub, sessions: BTreeSet<SessionId>) {
        use futures::StreamExt as _;

        let resolutions = sessions.into_iter().map(|session| async move {
            self.workspace(session)
                .await
                .map(|workspace| (workspace, session))
        });
        let mut open =
            futures::stream::iter(resolutions).buffer_unordered(WORKSPACE_RESOLUTION_CONCURRENCY);
        while let Some(resolved) = open.next().await {
            if let Some((workspace, session)) = resolved {
                hub.notify(ScopeKey::Session { workspace, session });
                hub.notify(ScopeKey::Workspace(workspace));
            }
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
            // The session-to-workspace binding is written once at session
            // creation and never changes, so a replica answer is as good as the
            // leader's; a miss on a just-created session only delays a latency
            // hint by one poll.
            .consistent_read(false)
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
    use std::sync::Arc;
    use std::time::Duration;

    use aex_observation_domain::keys::{BucketHour, ScopeKey, observation_pk};
    use aex_observation_domain::signal::Signal;
    use aex_wire::ids::{PrefixedId as _, SessionId, Uuid7, WorkspaceId};
    use aws_sdk_dynamodbstreams::types::{AttributeValue, Record, StreamRecord, StreamViewType};
    use aws_smithy_http_client::test_util::{ReplayEvent, StaticReplayClient, capture_request};
    use aws_smithy_types::body::SdkBody;
    use regional_observation_api::counters::{ReadCounter, ReadCounters};
    use regional_observation_api::wake::WakeHub;

    use super::{
        AuthorityStream, Backoff, RETRY_MIN, StreamSpec, WakeTarget, WorkspaceResolver, classify,
        read_shard,
    };

    #[test]
    fn backoff_escalates_to_its_ceiling_and_only_success_restores_the_floor() {
        let mut retry = Backoff::new();
        let observed: Vec<u64> = (0..7).map(|_| retry.failure().as_secs()).collect();
        assert_eq!(
            observed,
            [1, 2, 4, 8, 16, 30, 30],
            "failures escalate and hold the ceiling"
        );
        retry.success();
        assert_eq!(retry.failure(), RETRY_MIN);
    }

    fn event(status: u16, body: &str) -> ReplayEvent {
        ReplayEvent::new(
            http::Request::builder()
                .method("POST")
                .uri("https://streams.dynamodb.eu-west-1.amazonaws.com/")
                .body(SdkBody::empty())
                .expect("a request"),
            http::Response::builder()
                .status(status)
                .body(SdkBody::from(body.to_owned()))
                .expect("a response"),
        )
    }

    fn streams_client(events: Vec<ReplayEvent>) -> aws_sdk_dynamodbstreams::Client {
        let config = aws_sdk_dynamodbstreams::config::Builder::new()
            .behavior_version(aws_sdk_dynamodbstreams::config::BehaviorVersion::latest())
            .region(aws_sdk_dynamodbstreams::config::Region::new("eu-west-1"))
            .credentials_provider(aws_sdk_dynamodbstreams::config::Credentials::new(
                "AKIDTESTTESTTESTTEST",
                "test-secret",
                None,
                None,
                "aex-tests",
            ))
            .http_client(StaticReplayClient::new(events))
            .retry_config(aws_sdk_dynamodbstreams::config::retry::RetryConfig::disabled())
            .build();
        aws_sdk_dynamodbstreams::Client::from_conf(config)
    }

    /// The regression this pins: a persistent `GetRecords` failure must walk
    /// the backoff ladder even though every intervening `GetShardIterator`
    /// succeeds. The pre-fix shape reset the delay on acquisition, so three
    /// straight read failures slept 1+1+1 s for ever; with the reset placed
    /// after a successful read they sleep 1+2+4 s, and every failed provider
    /// call publishes one degradation.
    #[tokio::test(start_paused = true)]
    async fn a_persistent_record_read_failure_escalates_past_the_floor() {
        let iterator_ok = r#"{"ShardIterator":"iter"}"#;
        let read_failed =
            r#"{"__type":"com.amazonaws.dynamodb#InternalServerError","message":"unavailable"}"#;
        let client = streams_client(vec![
            event(200, iterator_ok),
            event(500, read_failed),
            event(200, iterator_ok),
            event(500, read_failed),
            event(200, iterator_ok),
            event(500, read_failed),
            event(200, iterator_ok),
            // A terminal page: records without a next iterator end the shard.
            event(200, r#"{"Records":[]}"#),
        ]);
        let (table_http, _requests) = capture_request(None);
        let table = aws_sdk_dynamodb::Client::from_conf(
            aws_sdk_dynamodb::config::Builder::new()
                .behavior_version(aws_sdk_dynamodb::config::BehaviorVersion::latest())
                .region(aws_sdk_dynamodb::config::Region::new("eu-west-1"))
                .credentials_provider(aws_sdk_dynamodb::config::Credentials::new(
                    "AKIDTESTTESTTESTTEST",
                    "test-secret",
                    None,
                    None,
                    "aex-tests",
                ))
                .http_client(table_http)
                .build(),
        );
        let counters = Arc::new(ReadCounters::default());

        let started = tokio::time::Instant::now();
        read_shard(
            client,
            StreamSpec {
                arn: "arn:aws:dynamodb:eu-west-1:000000000000:table/t/stream/1".to_owned(),
                authority: AuthorityStream::Session,
            },
            "shard-0".to_owned(),
            WakeHub::default(),
            WorkspaceResolver::new(table, "session-authority".to_owned()),
            Arc::clone(&counters),
        )
        .await;

        assert_eq!(
            started.elapsed(),
            Duration::from_secs(7),
            "three read failures back off 1+2+4 s; the pre-fix reset made this 3 s"
        );
        assert_eq!(counters.total(ReadCounter::WakeReaderDegraded), 3);
    }

    fn session(seed: u8) -> SessionId {
        SessionId::from_uuid7(Uuid7::compose(1, [seed; 10]))
    }

    #[tokio::test]
    async fn a_session_record_wakes_its_workspace_through_one_eventual_binding_read() {
        let session_id = session(3);
        let workspace_id = WorkspaceId::from_uuid7(Uuid7::compose(1, [7; 10]));
        let records = serde_json::json!({
            "Records": [{
                "dynamodb": {
                    "Keys": {
                        "pk": {"S": format!("SESSION#{session_id}")},
                        "sk": {"S": "EVT#00000000000000000001"}
                    },
                    "StreamViewType": "KEYS_ONLY"
                }
            }]
        })
        .to_string();
        let client = streams_client(vec![
            event(200, r#"{"ShardIterator":"iter"}"#),
            // A terminal page: records without a next iterator end the shard.
            event(200, &records),
        ]);

        let head = serde_json::json!({
            "Item": {
                "itemType": {"S": "session_head"},
                "workspaceId": {"S": workspace_id.to_string()}
            }
        })
        .to_string();
        let table_replay = StaticReplayClient::new(vec![ReplayEvent::new(
            http::Request::builder()
                .method("POST")
                .uri("https://dynamodb.eu-west-1.amazonaws.com/")
                .body(SdkBody::empty())
                .expect("a request"),
            http::Response::builder()
                .status(200)
                .body(SdkBody::from(head))
                .expect("a response"),
        )]);
        let table = aws_sdk_dynamodb::Client::from_conf(
            aws_sdk_dynamodb::config::Builder::new()
                .behavior_version(aws_sdk_dynamodb::config::BehaviorVersion::latest())
                .region(aws_sdk_dynamodb::config::Region::new("eu-west-1"))
                .credentials_provider(aws_sdk_dynamodb::config::Credentials::new(
                    "AKIDTESTTESTTESTTEST",
                    "test-secret",
                    None,
                    None,
                    "aex-tests",
                ))
                .http_client(table_replay.clone())
                .build(),
        );

        let hub = WakeHub::default();
        let mut session_wake = hub.subscribe(ScopeKey::Session {
            workspace: workspace_id,
            session: session_id,
        });
        let mut workspace_wake = hub.subscribe(ScopeKey::Workspace(workspace_id));

        read_shard(
            client,
            StreamSpec {
                arn: "arn:aws:dynamodb:eu-west-1:000000000000:table/t/stream/1".to_owned(),
                authority: AuthorityStream::Session,
            },
            "shard-0".to_owned(),
            hub.clone(),
            WorkspaceResolver::new(table, "session-authority".to_owned()),
            Arc::new(ReadCounters::default()),
        )
        .await;

        assert!(
            session_wake.wait(Duration::from_millis(1)).await,
            "the session scope is woken from the resolved binding: a scope names \
             its workspace, so an `EVT#` key alone cannot address one"
        );
        assert!(
            workspace_wake.wait(Duration::from_millis(1)).await,
            "the workspace scope is woken from the resolved binding"
        );

        let request = table_replay
            .actual_requests()
            .next()
            .expect("one binding read");
        let body: serde_json::Value =
            serde_json::from_slice(request.body().bytes().expect("the body is in memory"))
                .expect("the request body is JSON");
        assert_eq!(
            body["ConsistentRead"], false,
            "the session-to-workspace binding is immutable; a replica answer \
             is as good as the leader's"
        );
        assert_eq!(
            body["Key"]["pk"]["S"].as_str(),
            Some(format!("SESSION#{session_id}").as_str())
        );
        assert_eq!(body["Key"]["sk"]["S"], "HEAD");
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
        let scope = ScopeKey::Session {
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(1, [7; 10])),
            session: session(1),
        };
        let bucket = BucketHour::parse("2026-08-02T12").expect("bucket");
        let observation = record(
            observation_pk(&scope, Signal::Logs, bucket, 0),
            "00000000000000000001",
            StreamViewType::KeysOnly,
        );
        // An observation key names its own workspace, so the classifier returns
        // a complete scope and the reader needs no binding read for it.
        assert_eq!(
            classify(&observation, AuthorityStream::Observation),
            Some(WakeTarget::Scope(scope))
        );
        let event = record(
            format!("SESSION#{}", session(2)),
            "EVT#00000000000000000001",
            StreamViewType::KeysOnly,
        );
        // A session event names no workspace, so it can only be reported as a
        // session to resolve.
        assert_eq!(
            classify(&event, AuthorityStream::Session),
            Some(WakeTarget::Session(session(2)))
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
