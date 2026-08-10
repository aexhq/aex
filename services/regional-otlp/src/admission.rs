//! The bounded OTLP admission edge.
//!
//! The allocation gate is the point of this deployable. A request reserves its
//! worst-case decoded footprint **before the first decode byte**, and a
//! reservation that cannot be met inside the configured wait is an explicit
//! retryable `503` — not an unbounded allocation the process discovers as an
//! OOM kill. That, plus dedicated memory and reserved concurrency, is what makes
//! `reserved concurrency × decoded ceiling` the whole-region decode bound.

use std::sync::Arc;
use std::time::Duration;

use aex_observation_domain::canonical::{BatchBinding, CanonicalValue, batch_intent_digest};
use aex_observation_domain::keys::ScopeKey;
use aex_otlp_admission::{
    ContentCoding, DecodeRequest, MemoryBudget, OtlpEncoding, OtlpError, OtlpLimits, OtlpSignal,
    decode, normalize, reservation_for,
};
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::ids::{PrefixedId as _, SessionId, TelemetryBatchId, Uuid7};
use aex_wire::models::TelemetryAdmissionReceipt;
use aex_wire::routes::route;
use aex_wire::server::{OtlpApi, RequestContext};
use aex_wire::types::{DecimalU128, Timestamp};

use crate::authority::{AdmissionAuthority, AdmissionRequest, AuthorityError, PreparedObservation};
use crate::counters::{AdmissionCounter, AdmissionTelemetry};
use aex_observation_store_dynamodb::spool::GateState;
use aex_regional_http::context::RequestContext as EdgeContext;

/// How long the reservation loop sleeps between attempts.
const RESERVE_POLL: Duration = Duration::from_millis(2);

/// The composed admission service, shared across requests.
pub struct OtlpService {
    authority: AdmissionAuthority,
    limits: OtlpLimits,
    budget: MemoryBudget,
    reserve_wait: Duration,
    telemetry: AdmissionTelemetry,
}

impl std::fmt::Debug for OtlpService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OtlpService")
            .field("table", &self.authority.table())
            .field("bucket", &self.authority.bucket())
            .field("limits", &self.limits)
            .field("budget_bytes", &self.budget.capacity())
            .field("reserve_wait", &self.reserve_wait)
            .field("telemetry", &self.telemetry)
            .finish()
    }
}

impl OtlpService {
    /// Composes the service over its resolved adapters.
    #[must_use]
    pub fn new(
        authority: AdmissionAuthority,
        limits: OtlpLimits,
        budget: MemoryBudget,
        reserve_wait: Duration,
        telemetry: AdmissionTelemetry,
    ) -> Self {
        Self {
            authority,
            limits,
            budget,
            reserve_wait,
            telemetry,
        }
    }
}

/// One request's view of the service.
///
/// The generated trait hands a handler the body and nothing else, so the media
/// type the caller declared travels here rather than being sniffed out of the
/// bytes. Sniffing would make a mislabelled request succeed, which is exactly
/// the ambiguity a pinned protocol exists to remove.
#[derive(Clone)]
pub struct OtlpRequest {
    service: Arc<OtlpService>,
    authorized: EdgeContext,
    encoding: OtlpEncoding,
    coding: ContentCoding,
    session: Option<SessionId>,
}

impl std::fmt::Debug for OtlpRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OtlpRequest")
            .field("encoding", &self.encoding.as_str())
            .field("scope", &self.scope())
            .finish_non_exhaustive()
    }
}

impl OtlpRequest {
    /// Binds one request to the shared service.
    ///
    /// `session` is the caller's own claim, already parsed as a [`SessionId`] by
    /// [`crate::mount::session_id`] — the edge proves nothing about it and no
    /// lookup here checks it. What makes that safe is [`Self::scope`]: the
    /// session is only ever paired with the workspace the credential proved, and
    /// the rendered key leads with that workspace.
    #[must_use]
    pub fn new(
        service: Arc<OtlpService>,
        authorized: EdgeContext,
        encoding: OtlpEncoding,
        coding: ContentCoding,
        session: Option<SessionId>,
    ) -> Self {
        Self {
            service,
            authorized,
            encoding,
            coding,
            session,
        }
    }

    /// The scope this request's observations belong to.
    ///
    /// Two halves, and only one of them is the caller's. The workspace is the
    /// one the edge verified a credential for; the session is whatever the
    /// caller wrote in `aex-session-id`. They are combined, never substituted:
    /// a session-scoped row renders `S#{workspace}#{session}`, so the partition
    /// a caller can reach is bounded by the workspace it proved regardless of
    /// which session identifier it supplies. The worst a wrong id can do is file
    /// a batch against the caller's own wrong session — a customer's own
    /// problem, and reversible by them.
    ///
    /// This is why ingest needs no session→workspace lookup. Verifying a
    /// supplied identifier would put a read on the highest-volume path in the
    /// platform to establish a fact the key already encodes.
    #[must_use]
    pub fn scope(&self) -> ScopeKey {
        let workspace = self.authorized.auth.workspace_id;
        self.session
            .map_or(ScopeKey::Workspace(workspace), |session| {
                ScopeKey::Session { workspace, session }
            })
    }

    /// Admits one batch of one signal.
    // This is intentionally kept as one bounded admission transaction so the
    // reservation, decode, normalization and commit ordering is visible in one
    // place.
    #[allow(clippy::too_many_lines)]
    async fn admit(
        &self,
        cx: &RequestContext,
        signal: OtlpSignal,
        body: &[u8],
    ) -> WireResult<TelemetryAdmissionReceipt> {
        let service = &self.service;
        aex_otlp_admission::decode::check_encoded_size(body.len(), &service.limits)
            .map_err(|error| otlp_error(&error))?;
        // One scope decision per request, taken once and consumed by every
        // stage below. The normalized identity attributes, the batch binding
        // and the stored partition key cannot disagree about a session because
        // there is only one value for them to read.
        let scope_key = self.scope();

        // Step 1 — reserve before the first decode byte.
        let reservation = reservation_for(
            body.len(),
            self.coding.expansion_estimate(service.limits.max_ratio),
            service.limits.decoded_max,
        );
        let lease = reserve(&service.budget, reservation, service.reserve_wait).await?;

        // Steps 3 and 4 — decode and normalize under the reservation.
        let batch = decode(&DecodeRequest {
            signal,
            encoding: self.encoding,
            coding: self.coding,
            body,
            limits: &service.limits,
            lease: &lease,
        })
        .map_err(|error| otlp_error(&error))?;
        let scope = aex_otlp_admission::AuthenticatedScope {
            organization_id: self.authorized.auth.organization_id,
            workspace_id: self.authorized.auth.workspace_id,
            session_id: scope_key.session(),
            run_id: None,
            agent_id: None,
        };
        // The batch is admitted as its author wrote it. Nothing rewrites a
        // customer's bytes on this path: the platform's own credentials never
        // enter the sandbox that produced them, so there is no platform secret
        // in this telemetry for a filter to find.
        let normalized =
            normalize(&batch, &scope, &service.limits).map_err(|error| otlp_error(&error))?;

        // Step 5 — canonicalize and bind the batch.
        let mut observations = Vec::with_capacity(normalized.observations.len());
        for observation in &normalized.observations {
            observations.push(
                PreparedObservation::new(observation).map_err(|error| authority_error(&error))?,
            );
        }
        let descriptor = route(cx.route);
        let workspace = self.authorized.auth.workspace_id.to_string();
        let scope_text = scope_key.to_key();
        let principal = principal_id(cx);
        let bodies: Vec<CanonicalValue> = observations
            .iter()
            .map(|observation| observation.body.clone())
            .collect();
        let digest = batch_intent_digest(
            &BatchBinding {
                principal_id: &principal,
                method: descriptor.method.as_str(),
                canonical_route: descriptor.template,
                workspace_id: &workspace,
                scope: &scope_text,
            },
            &bodies,
        )
        .map_err(|error| {
            WireError::new(ErrorCode::InvalidTelemetry).with_message(error.to_string())
        })?;

        let now = Timestamp::from_datetime_trunc_ms(time::OffsetDateTime::now_utc())
            .map_err(|_| WireError::new(ErrorCode::InternalError))?;
        let request = AdmissionRequest {
            batch_id: batch_id_for(&digest),
            organization: self.authorized.auth.organization_id,
            workspace: self.authorized.auth.workspace_id,
            scope: scope_key,
            intent_digest: digest.to_string(),
            observations,
        };
        let accepted = normalized.observations.len();

        // The gate state is read here, through the authority's process-local
        // cache window, so a degraded admission can be counted per request;
        // the authority still refuses a closed gate on the state it is handed.
        let gate = service
            .authority
            .ingress_gate_cached()
            .await
            .map_err(|error| authority_error(&error))?;
        if matches!(gate, GateState::Degraded) {
            service
                .telemetry
                .count(AdmissionCounter::DegradedGateAdmission, 1);
        }

        let receipt = match service.authority.admit(&request, now, gate).await {
            Ok(receipt) => receipt,
            Err(error) => {
                if matches!(error, AuthorityError::MaterializeExhausted { .. }) {
                    service
                        .telemetry
                        .count(AdmissionCounter::MaterializeRetryExhausted, 1);
                }
                return Err(authority_error(&error));
            }
        };
        service
            .telemetry
            .count(AdmissionCounter::RecordsAdmitted, receipt.accepted);
        drop(lease);
        Ok(TelemetryAdmissionReceipt {
            accepted: DecimalU128::new(accepted as u128),
            accepted_at: receipt.accepted_at,
            batch_id: receipt.batch_id,
            bytes: DecimalU128::new(u128::from(receipt.logical_bytes)),
            // A batch either commits whole or is refused whole: there is no
            // OTLP partial success anywhere in this stream.
            rejected: DecimalU128::ZERO,
        })
    }
}

impl OtlpApi for OtlpRequest {
    async fn otlp_logs_ingest(
        &self,
        cx: &RequestContext,
        body: &[u8],
    ) -> WireResult<TelemetryAdmissionReceipt> {
        self.admit(cx, OtlpSignal::Logs, body).await
    }

    async fn otlp_metrics_ingest(
        &self,
        cx: &RequestContext,
        body: &[u8],
    ) -> WireResult<TelemetryAdmissionReceipt> {
        self.admit(cx, OtlpSignal::Metrics, body).await
    }

    async fn otlp_traces_ingest(
        &self,
        cx: &RequestContext,
        body: &[u8],
    ) -> WireResult<TelemetryAdmissionReceipt> {
        self.admit(cx, OtlpSignal::Traces, body).await
    }
}

/// Waits for a decode reservation, then fails retryably rather than allocating.
async fn reserve(
    budget: &MemoryBudget,
    bytes: usize,
    wait: Duration,
) -> WireResult<aex_otlp_admission::MemoryLease> {
    let deadline = std::time::Instant::now() + wait;
    loop {
        match budget.try_reserve(bytes) {
            Ok(lease) => return Ok(lease),
            Err(error) => {
                if bytes > budget.capacity() || std::time::Instant::now() >= deadline {
                    return Err(otlp_error(&error).with_retry_after(wait).with_message(
                        "decode memory is unavailable; retry after the advertised delay",
                    ));
                }
                tokio::time::sleep(RESERVE_POLL).await;
            }
        }
    }
}

/// Maps a decode failure onto the public vocabulary.
fn otlp_error(error: &OtlpError) -> WireError {
    let retryable = error.retryable();
    let wire = WireError::new(error.code()).with_message(error.to_string());
    if retryable {
        wire.with_retry_after(Duration::from_millis(
            aex_observation_domain::limits::OTLP_RESERVE_WAIT_MS,
        ))
    } else {
        wire
    }
}

/// Maps an authority failure onto the public vocabulary.
fn authority_error(error: &AuthorityError) -> WireError {
    match error {
        AuthorityError::GateClosed { .. } => WireError::new(ErrorCode::ObservabilityUnavailable)
            .with_retry_after(Duration::from_secs(30)),
        // The commit is durable and the batch identity content-addressed, so a
        // retry resumes materialization from the receipt.
        AuthorityError::MaterializeExhausted { .. } => {
            WireError::new(ErrorCode::ObservabilityUnavailable)
                .with_retry_after(Duration::from_secs(1))
        }
        AuthorityError::Fenced { .. } => WireError::new(ErrorCode::SessionDeleted),
        AuthorityError::IntentConflict { .. } => WireError::new(ErrorCode::IdempotencyConflict),
        AuthorityError::ClockSkew { .. } | AuthorityError::Malformed { .. } => {
            WireError::new(ErrorCode::InternalError)
        }
        AuthorityError::Provider { .. } | AuthorityError::Store(_) => {
            let wire = WireError::new(ErrorCode::ObservabilityUnavailable);
            if error.retryable() {
                wire.with_retry_after(Duration::from_secs(1))
            } else {
                WireError::new(ErrorCode::InvalidTelemetry).with_message(error.to_string())
            }
        }
    }
}

/// The stable principal identity the batch intent binds.
fn principal_id(cx: &RequestContext) -> String {
    serde_json::to_string(&cx.principal).unwrap_or_else(|_| "unknown".to_owned())
}

/// Derives the batch identity from the batch intent digest.
///
/// Content-addressed on purpose. An exporter retry after a lost response — and
/// a retry of this service's own retryable `503` — re-presents the same bound
/// content, so deriving the identity from the intent digest makes every
/// equal-content retry converge on one `(workspace, batchId)` receipt row.
/// That convergence is what makes the staged-commit idempotency machinery
/// reachable at all: a clock-minted id gave every retry a fresh receipt, new
/// sequences and undedupable duplicates.
///
/// The value still parses as a `UUIDv7`, but its embedded 48-bit timestamp is
/// digest bytes, not a clock reading. Nothing reads it as time or as an order:
/// the receipt and staging rows key on the `BATCH#` partition alone, the spool
/// sort key orders by the admission wall clock, and the observation order
/// tuple (`aex-observation-domain::order`) uses ids only as a tie-break after
/// its own time component. `stable_observation_id` inherits the same
/// pseudo-time, with the same non-dependence.
fn batch_id_for(digest: &aex_wire::idempotency::IntentDigest) -> TelemetryBatchId {
    let bytes = digest.as_bytes();
    let mut millis = 0u64;
    for byte in &bytes[..6] {
        millis = (millis << 8) | u64::from(*byte);
    }
    let mut entropy = [0u8; 10];
    entropy.copy_from_slice(&bytes[6..16]);
    TelemetryBatchId::from_uuid7(Uuid7::compose(millis, entropy))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use aex_observation_domain::canonical::{BatchBinding, CanonicalValue, batch_intent_digest};
    use aex_observation_domain::keys::ScopeKey;
    use aex_otlp_admission::{ContentCoding, MemoryBudget, OtlpEncoding, OtlpError, OtlpLimits};
    use aex_wire::error::ErrorCode;
    use aex_wire::idempotency::PrincipalScope;
    use aex_wire::ids::{ApiKeyId, OrganizationId, PrefixedId as _, SessionId, WorkspaceId};

    use super::{OtlpRequest, OtlpService, authority_error, batch_id_for, otlp_error, reserve};
    use crate::authority::{AdmissionAuthority, AuthorityError};
    use crate::counters::AdmissionTelemetry;

    #[test]
    fn an_oversize_body_is_a_four_one_three_before_any_allocation() {
        let error = otlp_error(&OtlpError::EncodedTooLarge {
            observed: 8 * 1024 * 1024,
            limit: 4 * 1024 * 1024,
        });
        assert_eq!(error.code, ErrorCode::TelemetryPayloadTooLarge);
        assert!(error.retry_after.is_none());
    }

    #[test]
    fn a_memory_refusal_is_an_explicit_retryable_failure() {
        let error = otlp_error(&OtlpError::MemoryUnavailable { requested: 1 });
        assert_eq!(error.code, ErrorCode::ObservabilityUnavailable);
        assert!(
            error.retry_after.is_some(),
            "an overload refusal must advertise its retry delay"
        );
    }

    #[tokio::test]
    async fn a_request_larger_than_the_whole_budget_fails_before_it_waits() {
        let budget = MemoryBudget::new(1_024);
        let started = std::time::Instant::now();
        let error = reserve(&budget, 4_096, Duration::from_millis(500))
            .await
            .expect_err("a request larger than the budget can never succeed");
        assert_eq!(error.code, ErrorCode::ObservabilityUnavailable);
        assert!(
            started.elapsed() < Duration::from_millis(400),
            "it must not spin against a reservation that can never be met"
        );
        assert_eq!(budget.reserved(), 0, "no permits leaked");
    }

    #[tokio::test]
    async fn a_lease_is_released_and_the_budget_recovers() {
        let budget = MemoryBudget::new(4_096);
        {
            let lease = reserve(&budget, 4_096, Duration::from_millis(5))
                .await
                .expect("the whole budget is free");
            assert_eq!(budget.reserved(), 4_096);
            assert!(
                reserve(&budget, 1, Duration::from_millis(5)).await.is_err(),
                "an exhausted budget refuses rather than allocating"
            );
            drop(lease);
        }
        assert_eq!(budget.reserved(), 0);
    }

    #[test]
    fn a_closed_gate_is_a_retryable_five_zero_three() {
        let error = authority_error(&AuthorityError::GateClosed { state: "closed" });
        assert_eq!(error.code, ErrorCode::ObservabilityUnavailable);
        assert!(error.retry_after.is_some());
    }

    #[test]
    fn a_reused_batch_with_another_intent_is_a_conflict() {
        let error = authority_error(&AuthorityError::IntentConflict {
            batch: "bch_x".to_owned(),
        });
        assert_eq!(error.code, ErrorCode::IdempotencyConflict);
    }

    #[test]
    fn exhausted_materialization_is_a_retryable_five_zero_three() {
        let error = AuthorityError::MaterializeExhausted { attempts: 9 };
        assert!(
            error.retryable(),
            "the receipt is durable; a retry resumes materialization"
        );
        let wire = authority_error(&error);
        assert_eq!(wire.code, ErrorCode::ObservabilityUnavailable);
        assert!(wire.retry_after.is_some());
    }

    /// The binding of one fixture batch.
    fn binding() -> BatchBinding<'static> {
        BatchBinding {
            principal_id: "{\"kind\":\"workspaceKey\"}",
            method: "POST",
            canonical_route: "/api/otlp/v1/logs",
            workspace_id: "wsp_01h455vb4pex5vsknk084sn02q",
            scope: "WS#wsp_01h455vb4pex5vsknk084sn02q",
        }
    }

    #[test]
    fn an_equal_content_retry_converges_on_one_batch_identity() {
        // The whole idempotency guarantee hangs on this derivation: the same
        // bound content — same principal, route, workspace, scope and bodies —
        // must resolve to the same `(workspace, batchId)` receipt on every
        // retry, and different content must not.
        let bodies = vec![CanonicalValue::Str("one log line".into())];
        let first = batch_intent_digest(&binding(), &bodies).expect("a digest");
        let second = batch_intent_digest(&binding(), &bodies).expect("a digest");
        assert_eq!(batch_id_for(&first), batch_id_for(&second));

        let other_bodies = vec![CanonicalValue::Str("a different line".into())];
        let different = batch_intent_digest(&binding(), &other_bodies).expect("a digest");
        assert_ne!(batch_id_for(&first), batch_id_for(&different));

        let reordered =
            batch_intent_digest(&binding(), &[other_bodies[0].clone(), bodies[0].clone()])
                .expect("a digest");
        assert_ne!(
            batch_id_for(&different),
            batch_id_for(&reordered),
            "the observation list is ordered; reordering is a different batch"
        );
    }

    /// The workspace every fixture request is authenticated for.
    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(aex_wire::Uuid7::compose(1, [2; 10]))
    }

    /// The session a caller would have named, had there been anywhere to name
    /// one.
    fn session() -> SessionId {
        SessionId::from_uuid7(aex_wire::Uuid7::compose(1, [4; 10]))
    }

    /// One verified edge context for a workspace API key.
    ///
    /// This is the whole input a request's attribution is allowed to depend on:
    /// what the edge proved, and nothing the caller wrote.
    fn authorized() -> aex_regional_http::context::RequestContext {
        use aex_regional_http::context::{
            AccountState, AuthorizationEpochs, EffectiveLimits, RegionalAuthorization,
            RequestContext,
        };
        let organization = OrganizationId::from_uuid7(aex_wire::Uuid7::compose(1, [5; 10]));
        RequestContext {
            request_id: aex_wire::types::RequestId::parse("0123456789abcdef0123456789abcdef")
                .expect("a diagnostic id"),
            route: aex_wire::routes::RouteId::OtlpLogsIngest,
            auth: RegionalAuthorization {
                principal: PrincipalScope::WorkspaceKey {
                    key: ApiKeyId::from_uuid7(aex_wire::Uuid7::compose(1, [6; 10])),
                    workspace: workspace(),
                    organization,
                },
                credential_binding: [7; 32],
                organization_id: organization,
                workspace_id: workspace(),
                placement: aex_wire::types::Region::EuWest1,
                scopes: aex_wire::scopes::ScopeSet::empty(),
                account_state: AccountState::Active,
                epochs: AuthorizationEpochs::default(),
            },
            limits: EffectiveLimits {
                json_body_bytes: 1_048_576,
                otlp_body_bytes: 4 * 1_024 * 1_024,
                query_page_items: 100,
                query_page_bytes: 1_048_576,
            },
            operation_id: None,
            idempotency: None,
            if_match: None,
            received_at: time::OffsetDateTime::UNIX_EPOCH,
        }
    }

    /// One request whose providers would fail loudly if they were used.
    ///
    /// The replay client is handed **no** events, so any provider call at all is
    /// an error rather than a silently satisfied read. The scope derivation
    /// these cases assert is therefore proven to be a pure function of the
    /// verified credential and the supplied header, with **no read behind it** —
    /// which is the whole point: the ingest path may not spend a
    /// session→workspace lookup to decide where a batch is filed.
    fn request_with_unusable_provider(session: Option<SessionId>) -> OtlpRequest {
        use aws_smithy_http_client::test_util::StaticReplayClient;
        let replay = StaticReplayClient::new(Vec::new());
        let dynamodb = aws_sdk_dynamodb::Client::from_conf(
            aws_sdk_dynamodb::Config::builder()
                .behavior_version(aws_sdk_dynamodb::config::BehaviorVersion::latest())
                .region(aws_sdk_dynamodb::config::Region::new("eu-west-1"))
                .credentials_provider(aws_sdk_dynamodb::config::Credentials::new(
                    "AKIDTESTTESTTESTTEST",
                    "test-secret",
                    None,
                    None,
                    "aex-tests",
                ))
                .http_client(replay.clone())
                .build(),
        );
        let s3 = aws_sdk_s3::Client::from_conf(
            aws_sdk_s3::Config::builder()
                .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
                .region(aws_sdk_s3::config::Region::new("eu-west-1"))
                .build(),
        );
        let region = aex_wire::types::Region::from_name("eu-west-1").expect("a region");
        let service = OtlpService::new(
            AdmissionAuthority::new(
                dynamodb,
                s3,
                "test-observation-authority",
                "test-observation-bucket",
                region,
            ),
            OtlpLimits::REGISTERED,
            MemoryBudget::new(1024 * 1024),
            Duration::from_millis(5),
            AdmissionTelemetry::new(
                aex_platform_telemetry::Handle::install(
                    &aex_platform_telemetry::Settings::default(),
                    None,
                ),
                "dev",
                "eu-west-1",
            ),
        );
        OtlpRequest::new(
            Arc::new(service),
            authorized(),
            OtlpEncoding::Protobuf,
            ContentCoding::Identity,
            session,
        )
    }

    #[test]
    fn a_batch_with_no_supplied_session_is_workspace_scoped() {
        let request = request_with_unusable_provider(None);
        assert_eq!(
            request.scope(),
            ScopeKey::Workspace(workspace()),
            "an absent `aex-session-id` means workspace scope"
        );
        assert_eq!(request.scope().session(), None);
        assert_eq!(request.scope().to_key(), format!("W#{}", workspace()));
    }

    #[test]
    fn a_supplied_session_is_attributed_inside_the_credentials_workspace_and_nowhere_else() {
        // The session here is entirely the caller's word — nothing verified it
        // and nothing looked it up. What bounds it is the other half of the key.
        let request = request_with_unusable_provider(Some(session()));
        assert_eq!(
            request.scope(),
            ScopeKey::Session {
                workspace: workspace(),
                session: session(),
            },
            "the supplied session is combined with the proven workspace"
        );
        assert_eq!(request.scope().session(), Some(session()));
        assert_eq!(
            request.scope().workspace(),
            workspace(),
            "the tenant of the written partition is the one the edge verified"
        );

        let key = request.scope().to_key();
        assert_eq!(key, format!("S#{}#{}", workspace(), session()));
        assert!(
            key.starts_with(&format!("S#{}#", workspace())),
            "every partition this batch writes is prefixed by its own workspace"
        );

        // The same identifier presented against another credential lands in a
        // different partition, so no session id is a route into another tenant.
        let stranger = ScopeKey::Session {
            workspace: WorkspaceId::from_uuid7(aex_wire::Uuid7::compose(1, [9; 10])),
            session: session(),
        };
        assert_ne!(request.scope().to_key(), stranger.to_key());
        let bucket = aex_observation_domain::keys::BucketHour::parse("2026-08-01T09")
            .expect("a fixture bucket");
        let signal = aex_observation_domain::signal::Signal::Logs;
        assert_ne!(
            aex_observation_domain::keys::observation_pk(&request.scope(), signal, bucket, 0),
            aex_observation_domain::keys::observation_pk(&stranger, signal, bucket, 0),
            "one session id must not reach two workspaces' observation partitions"
        );
    }

    #[test]
    fn a_derived_batch_identity_is_a_parseable_uuid7() {
        // `Uuid7::compose` forces the version and variant nibbles, so a digest
        // -derived identity must survive the same encode/parse round trip a
        // clock-minted one did.
        let digest =
            batch_intent_digest(&binding(), &[CanonicalValue::Str("x".into())]).expect("a digest");
        let id = batch_id_for(&digest);
        let parsed = aex_wire::ids::TelemetryBatchId::parse(id.encode().as_str())
            .expect("a derived identity round-trips");
        assert_eq!(parsed, id);
    }
}
