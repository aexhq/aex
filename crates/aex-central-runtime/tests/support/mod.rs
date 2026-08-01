//! Shared fixtures for the `aex-central-runtime` test targets.

#![allow(dead_code, reason = "each test target uses a different subset")]
#![allow(missing_docs, reason = "the module doc states what these fixtures are")]

use std::sync::{Arc, Mutex};

use aex_central_runtime::pepper::{PepperDirectory, PepperRecord, PepperState};
use aex_identity_app::ports::{PepperPurpose, StoreError};
use aex_identity_domain::PepperVersion;
use async_trait::async_trait;
use aws_smithy_http_client::test_util::{ReplayEvent, StaticReplayClient};
use aws_smithy_types::body::SdkBody;

/// The 32 zero-to-thirty-one bytes, base64 with the standard alphabet.
pub const MATERIAL_B64: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
/// A second, distinct 32 bytes.
pub const OTHER_MATERIAL_B64: &str = "fx4dHBsaGRgXFhUUExIREA8ODQwLCgkIBwYFBAMCAQA=";

/// The secret id both central peppers are configured with.
pub const SECRET_ID: &str = "aex/dev/identity-pepper";

/// The Secrets Manager version id one pepper row points at.
pub const VERSION_ID: &str = "11111111-2222-3333-4444-555555555555";

#[must_use]
pub fn payload(version: u32, purpose: &str, material: &str) -> String {
    format!(r#"{{"version":{version},"purpose":"{purpose}","secret":"{material}"}}"#)
}

/// One `GetSecretValue` answer carrying `body` as the secret string.
#[must_use]
pub fn secret_response(version_id: &str, body: &str) -> String {
    let escaped = body.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        r#"{{"ARN":"arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex","Name":"aex","VersionId":"{version_id}","SecretString":"{escaped}","VersionStages":["AWSCURRENT"],"CreatedDate":1.0}}"#
    )
}

/// A Secrets Manager client answering a scripted sequence.
#[must_use]
pub fn secrets_client(
    answers: Vec<(u16, String)>,
) -> (aws_sdk_secretsmanager::Client, StaticReplayClient) {
    let events: Vec<ReplayEvent> = answers
        .into_iter()
        .map(|(status, body)| {
            ReplayEvent::new(
                http::Request::builder()
                    .method("POST")
                    .uri("https://secretsmanager.eu-west-1.amazonaws.com/")
                    .body(SdkBody::empty())
                    .expect("a request"),
                http::Response::builder()
                    .status(status)
                    .body(SdkBody::from(body))
                    .expect("a response"),
            )
        })
        .collect();
    let replay = StaticReplayClient::new(events);
    let config = aws_sdk_secretsmanager::Config::builder()
        .behavior_version(aws_sdk_secretsmanager::config::BehaviorVersion::latest())
        .region(aws_sdk_secretsmanager::config::Region::new("eu-west-1"))
        .credentials_provider(aws_sdk_secretsmanager::config::Credentials::new(
            "AKIDTESTTESTTESTTEST",
            "test-secret",
            None,
            None,
            "aex-tests",
        ))
        .http_client(replay.clone())
        // No retries. A scripted suite fixes what one call does; a retry would
        // answer the second attempt from the next event and hide the mapping
        // under test.
        .retry_config(aws_sdk_secretsmanager::config::retry::RetryConfig::disabled())
        .build();
    (aws_sdk_secretsmanager::Client::from_conf(config), replay)
}

/// One scripted `Invoke` answer: status, extra headers, body.
pub type LambdaAnswer = (u16, Vec<(&'static str, &'static str)>, String);

/// No extra headers.
#[must_use]
pub fn plain() -> Vec<(&'static str, &'static str)> {
    Vec::new()
}

/// The header Lambda sets when the handler itself raised.
#[must_use]
pub fn function_error() -> Vec<(&'static str, &'static str)> {
    vec![("X-Amz-Function-Error", "Unhandled")]
}

/// The header the `restJson1` protocol names a modelled error with.
#[must_use]
pub fn error_type(name: &'static str) -> Vec<(&'static str, &'static str)> {
    vec![("X-Amzn-Errortype", name)]
}

/// A Lambda client answering a scripted sequence of `Invoke` responses.
#[must_use]
pub fn lambda_client(answers: Vec<LambdaAnswer>) -> (aws_sdk_lambda::Client, StaticReplayClient) {
    let events: Vec<ReplayEvent> = answers
        .into_iter()
        .map(|(status, headers, body)| {
            let mut response = http::Response::builder().status(status);
            for (name, value) in headers {
                response = response.header(name, value);
            }
            ReplayEvent::new(
                http::Request::builder()
                    .method("POST")
                    .uri("https://lambda.eu-west-1.amazonaws.com/")
                    .body(SdkBody::empty())
                    .expect("a request"),
                response.body(SdkBody::from(body)).expect("a response"),
            )
        })
        .collect();
    let replay = StaticReplayClient::new(events);
    let config = aws_sdk_lambda::Config::builder()
        .behavior_version(aws_sdk_lambda::config::BehaviorVersion::latest())
        .region(aws_sdk_lambda::config::Region::new("eu-west-1"))
        .credentials_provider(aws_sdk_lambda::config::Credentials::new(
            "AKIDTESTTESTTESTTEST",
            "test-secret",
            None,
            None,
            "aex-tests",
        ))
        .http_client(replay.clone())
        // Same reason as the Secrets Manager fixture.
        .retry_config(aws_sdk_lambda::config::retry::RetryConfig::disabled())
        .build();
    (aws_sdk_lambda::Client::from_conf(config), replay)
}

/// The wire identifier for a stored workspace id.
///
/// # Panics
///
/// If `id` is not a `UUIDv7`, which every AEX row id is.
#[must_use]
pub fn workspace_id(id: uuid::Uuid) -> aex_wire::ids::WorkspaceId {
    use aex_wire::ids::PrefixedId as _;
    aex_wire::ids::WorkspaceId::from_uuid7(
        aex_wire::ids::Uuid7::from_bytes(*id.as_bytes()).expect("a UUIDv7"),
    )
}

/// One region's `workspace_provisioned` answer, encoded exactly as the wire.
///
/// # Panics
///
/// If the envelope does not encode, which would be a contract defect.
#[must_use]
pub fn provisioned(id: uuid::Uuid, created: bool) -> String {
    use aex_wire::ids::PrefixedId as _;
    let workspace = workspace_id(id);
    envelope(
        workspace.uuid7(),
        aex_internal_contracts::control::RegionalControlOutcome::WorkspaceProvisioned {
            workspace,
            created,
        },
    )
}

/// One region's `workspace_deleted` answer.
///
/// # Panics
///
/// If the envelope does not encode.
#[must_use]
pub fn deleted(id: uuid::Uuid, removed: bool) -> String {
    use aex_wire::ids::PrefixedId as _;
    let workspace = workspace_id(id);
    envelope(
        workspace.uuid7(),
        aex_internal_contracts::control::RegionalControlOutcome::WorkspaceDeleted {
            workspace,
            removed,
        },
    )
}

/// One region's refusal.
///
/// # Panics
///
/// If the envelope does not encode.
#[must_use]
pub fn refused(id: uuid::Uuid, reason: aex_internal_contracts::control::RegionalRefusal) -> String {
    use aex_wire::ids::PrefixedId as _;
    envelope(
        workspace_id(id).uuid7(),
        aex_internal_contracts::control::RegionalControlOutcome::Refused { reason },
    )
}

/// Wraps one outcome in the versioned envelope a region answers with.
///
/// # Panics
///
/// If the envelope does not encode.
#[must_use]
pub fn envelope(
    request_id: aex_wire::Uuid7,
    payload: aex_internal_contracts::control::RegionalControlOutcome,
) -> String {
    serde_json::to_string(&aex_internal_contracts::control::RegionalControlEnvelope {
        schema_version: aex_internal_contracts::SchemaVersion::V1,
        request_id,
        payload,
    })
    .expect("the envelope encodes")
}

/// A pepper directory that answers from memory and counts its reads.
///
/// Its `Debug` renders counts only, for the same reason the real one will: a
/// directory is embedded in the keystore's own rendering, and a `secret_ref` is
/// a live Secrets Manager version id rather than something to print into a log.
pub struct FakeDirectory {
    rows: Mutex<Vec<PepperRecord>>,
    failure: Mutex<Option<StoreError>>,
    reads: Mutex<usize>,
}

impl std::fmt::Debug for FakeDirectory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FakeDirectory")
            .field("rows", &self.rows.lock().map_or(0, |rows| rows.len()))
            .finish_non_exhaustive()
    }
}

impl FakeDirectory {
    #[must_use]
    pub fn with(rows: Vec<PepperRecord>) -> Arc<Self> {
        Arc::new(Self {
            rows: Mutex::new(rows),
            failure: Mutex::new(None),
            reads: Mutex::new(0),
        })
    }

    #[must_use]
    pub fn one(version: u16, state: PepperState, secret_ref: &str) -> Arc<Self> {
        Self::with(vec![PepperRecord {
            version: PepperVersion::new(version),
            purpose: PepperPurpose::Identity,
            state,
            secret_ref: secret_ref.to_owned(),
        }])
    }

    pub fn fails_with(&self, error: StoreError) {
        *self.failure.lock().expect("not poisoned") = Some(error);
    }

    #[must_use]
    pub fn reads(&self) -> usize {
        *self.reads.lock().expect("not poisoned")
    }

    fn record(&self) {
        *self.reads.lock().expect("not poisoned") += 1;
    }

    fn refusal(&self) -> Option<StoreError> {
        self.failure.lock().expect("not poisoned").clone()
    }
}

#[async_trait]
impl PepperDirectory for FakeDirectory {
    async fn active(&self, purpose: PepperPurpose) -> Result<PepperRecord, StoreError> {
        self.record();
        if let Some(error) = self.refusal() {
            return Err(error);
        }
        self.rows
            .lock()
            .expect("not poisoned")
            .iter()
            .find(|row| row.purpose == purpose && row.state == PepperState::Active)
            .cloned()
            .ok_or(StoreError::NotFound)
    }

    async fn by_version(
        &self,
        purpose: PepperPurpose,
        version: PepperVersion,
    ) -> Result<PepperRecord, StoreError> {
        self.record();
        if let Some(error) = self.refusal() {
            return Err(error);
        }
        self.rows
            .lock()
            .expect("not poisoned")
            .iter()
            .find(|row| row.purpose == purpose && row.version == version)
            .cloned()
            .ok_or(StoreError::NotFound)
    }
}

/// Runs one future on a current-thread runtime.
pub fn run<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime")
        .block_on(future)
}
