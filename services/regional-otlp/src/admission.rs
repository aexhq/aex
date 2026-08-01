//! The bounded OTLP admission edge.
//!
//! The allocation gate is the point of this deployable. A request reserves its
//! worst-case decoded footprint **before the first decode byte**, and a
//! reservation that cannot be met inside the configured wait is an explicit
//! retryable `503` — not an unbounded allocation the process discovers as an
//! OOM kill. That, plus dedicated memory and reserved concurrency, is what makes
//! `reserved concurrency × decoded ceiling` the whole-region decode bound.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use aex_observation_domain::canonical::{BatchBinding, CanonicalValue, batch_intent_digest};
use aex_observation_domain::keys::ScopeKey;
use aex_otlp_admission::{
    ContentCoding, DecodeRequest, DigestRedactor, ManagedSecretRedactor, MemoryBudget,
    NoManagedSecrets, NormalizedBatch, OtlpEncoding, OtlpError, OtlpLimits, OtlpSignal,
    RedactionReport, SecretDigest, SecretDigestManifest, decode, normalize, reservation_for,
};
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::ids::{PrefixedId as _, SessionId, TelemetryBatchId, Uuid7};
use aex_wire::models::TelemetryAdmissionReceipt;
use aex_wire::routes::route;
use aex_wire::server::{OtlpApi, RequestContext};
use aex_wire::types::{DecimalU128, Timestamp};
use aws_sdk_dynamodb::types::AttributeValue;

use crate::authority::{AdmissionAuthority, AdmissionRequest, AuthorityError, PreparedObservation};
use crate::edge::Authorized;

/// The header naming the session an in-guest collector is emitting for.
pub const SESSION_HEADER: &str = "aex-session-id";

/// How long the reservation loop sleeps between attempts.
const RESERVE_POLL: Duration = Duration::from_millis(2);

/// The composed admission service, shared across requests.
pub struct OtlpService {
    authority: AdmissionAuthority,
    custody: CustodyManifests,
    limits: OtlpLimits,
    budget: MemoryBudget,
    reserve_wait: Duration,
    redaction_key: Vec<u8>,
}

impl std::fmt::Debug for OtlpService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OtlpService")
            .field("table", &self.authority.table())
            .field("bucket", &self.authority.bucket())
            .field("custody_table", &self.custody)
            .field("limits", &self.limits)
            .field("budget_bytes", &self.budget.capacity())
            .field("reserve_wait", &self.reserve_wait)
            .field("redaction_key", &"<redacted>")
            .finish()
    }
}

impl OtlpService {
    /// Composes the service over its resolved adapters.
    #[must_use]
    pub fn new(
        authority: AdmissionAuthority,
        custody: CustodyManifests,
        limits: OtlpLimits,
        budget: MemoryBudget,
        reserve_wait: Duration,
        redaction_key: Vec<u8>,
    ) -> Self {
        Self {
            authority,
            custody,
            limits,
            budget,
            reserve_wait,
            redaction_key,
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
    authorized: Authorized,
    session: Option<SessionId>,
    encoding: OtlpEncoding,
    coding: ContentCoding,
}

impl std::fmt::Debug for OtlpRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OtlpRequest")
            .field("encoding", &self.encoding.as_str())
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

impl OtlpRequest {
    /// Binds one request to the shared service.
    #[must_use]
    pub const fn new(
        service: Arc<OtlpService>,
        authorized: Authorized,
        session: Option<SessionId>,
        encoding: OtlpEncoding,
        coding: ContentCoding,
    ) -> Self {
        Self {
            service,
            authorized,
            session,
            encoding,
            coding,
        }
    }

    /// The scope this request's observations belong to.
    #[must_use]
    pub fn scope(&self) -> ScopeKey {
        self.session.map_or(
            ScopeKey::Workspace(self.authorized.workspace),
            ScopeKey::Session,
        )
    }

    /// Admits one batch of one signal.
    async fn admit(
        &self,
        cx: &RequestContext,
        signal: OtlpSignal,
        body: &[u8],
    ) -> WireResult<TelemetryAdmissionReceipt> {
        let service = &self.service;
        aex_otlp_admission::decode::check_encoded_size(body.len(), &service.limits)
            .map_err(|error| otlp_error(&error))?;

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
            organization_id: self.authorized.organization,
            workspace_id: self.authorized.workspace,
            session_id: self.session,
            run_id: None,
            agent_id: None,
        };
        let mut normalized =
            normalize(&batch, &scope, &service.limits).map_err(|error| otlp_error(&error))?;

        // Step 4b — redact platform-injected secrets with zero decrypt permission.
        self.redact(&mut normalized).await?;

        // Step 5 — canonicalize and bind the batch.
        let mut observations = Vec::with_capacity(normalized.observations.len());
        for observation in &normalized.observations {
            observations.push(
                PreparedObservation::new(observation).map_err(|error| authority_error(&error))?,
            );
        }
        let descriptor = route(cx.route);
        let scope_key = self.scope();
        let workspace = self.authorized.workspace.to_string();
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
            batch_id: mint_batch_id(),
            organization: self.authorized.organization,
            workspace: self.authorized.workspace,
            scope: scope_key,
            intent_digest: digest.to_string(),
            observations,
        };
        let accepted = normalized.observations.len();
        let receipt = service
            .authority
            .admit(&request, now)
            .await
            .map_err(|error| authority_error(&error))?;
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

    /// Removes AEX-managed secret values the platform itself injected.
    async fn redact(&self, batch: &mut NormalizedBatch) -> WireResult<RedactionReport> {
        let Some(session) = self.session else {
            // Only a session can have had a platform secret injected into it. A
            // workspace-key batch is customer-authored throughout, and the
            // product never claims arbitrary customer bytes can be recognised.
            return apply(&NoManagedSecrets, batch).map_err(|error| otlp_error(&error));
        };
        let manifest = self
            .service
            .custody
            .manifest(session)
            .await
            .map_err(|error| authority_error(&error))?;
        if manifest.entries.is_empty() {
            return apply(&NoManagedSecrets, batch).map_err(|error| otlp_error(&error));
        }
        let redactor = DigestRedactor::new(
            self.service.redaction_key.clone(),
            manifest,
            self.service.limits.redact_budget_bytes,
        );
        // Budget exhaustion fails the batch closed: admitting unredacted bytes
        // is never an acceptable degradation.
        apply(&redactor, batch).map_err(|error| otlp_error(&error))
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

/// Applies one redactor to every canonical value in the batch.
fn apply<R: ManagedSecretRedactor + ?Sized>(
    redactor: &R,
    batch: &mut NormalizedBatch,
) -> Result<RedactionReport, OtlpError> {
    let mut report = RedactionReport::default();
    for observation in &mut batch.observations {
        report.merge(redactor.redact(&mut observation.body)?);
        let keys: Vec<String> = observation.attr_s.keys().cloned().collect();
        for key in keys {
            let Some(current) = observation.attr_s.get(&key) else {
                continue;
            };
            let mut value = CanonicalValue::Str(current.clone().into_boxed_str());
            report.merge(redactor.redact(&mut value)?);
            if let CanonicalValue::Str(text) = value {
                observation.attr_s.insert(key, text.into_string());
            }
        }
    }
    Ok(report)
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
    use aex_otlp_admission::AdmissionCode;

    let retryable = error.retryable();
    let wire = match error.code() {
        AdmissionCode::Registered(code) => WireError::new(code),
        // A pending code has no registered spelling, so it is reported under the
        // nearest registered code with the pending spelling in the message
        // rather than silently under a code that means something else.
        AdmissionCode::Pending(pending) => WireError::new(ErrorCode::InvalidTelemetry)
            .with_message(format!("{} ({})", error, pending.as_str())),
    };
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

/// Mints one batch identity.
fn mint_batch_id() -> TelemetryBatchId {
    let bytes = *uuid::Uuid::now_v7().as_bytes();
    let id = Uuid7::from_bytes(bytes).unwrap_or_else(|_| Uuid7::compose(0, [0u8; 10]));
    TelemetryBatchId::from_uuid7(id)
}

/// The `regional-secret-custody` redaction manifest reader.
///
/// **Zero decrypt permission.** The manifest is a keyed-digest directory, so
/// this deployable can recognise an injected secret without ever being able to
/// read one. The item family is the cross-stream contract the regional-secret
/// stream publishes; an absent manifest means no managed secret was injected
/// into the session, which is the only reading that does not invent one.
#[derive(Clone, Debug)]
pub struct CustodyManifests {
    dynamodb: aws_sdk_dynamodb::Client,
    table: String,
}

impl CustodyManifests {
    /// Binds the reader to the custody table.
    #[must_use]
    pub fn new(dynamodb: aws_sdk_dynamodb::Client, table: impl Into<String>) -> Self {
        Self {
            dynamodb,
            table: table.into(),
        }
    }

    /// The partition key of one session's manifest.
    #[must_use]
    pub fn partition_key(session: SessionId) -> String {
        format!("REDACT#{session}")
    }

    /// Reads one session's manifest.
    ///
    /// # Errors
    ///
    /// Returns [`AuthorityError::Provider`] when the read fails. The batch then
    /// fails closed: admitting unredacted bytes is never an acceptable
    /// degradation.
    pub async fn manifest(
        &self,
        session: SessionId,
    ) -> Result<SecretDigestManifest, AuthorityError> {
        let response = self
            .dynamodb
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(Self::partition_key(session)))
            .key("sk", AttributeValue::S("MANIFEST".to_owned()))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| AuthorityError::Provider {
                operation: "GetItem",
                reason: error.to_string(),
            })?;
        let Some(item) = response.item else {
            return Ok(SecretDigestManifest {
                session,
                custody_revision: 0,
                entries: Vec::new(),
            });
        };
        Ok(SecretDigestManifest {
            session,
            custody_revision: item
                .get("custodyRevision")
                .and_then(|value| value.as_n().ok())
                .and_then(|text| text.parse().ok())
                .unwrap_or(0),
            entries: parse_entries(&item),
        })
    }
}

/// Reads the `{len, hmac}` rows of one manifest item.
fn parse_entries(item: &HashMap<String, AttributeValue>) -> Vec<SecretDigest> {
    let Some(list) = item.get("entries").and_then(|value| value.as_l().ok()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|entry| {
            let map = entry.as_m().ok()?;
            let len = map
                .get("len")
                .and_then(|value| value.as_n().ok())
                .and_then(|text| text.parse::<u16>().ok())?;
            let hmac = map
                .get("hmac")
                .and_then(|value| value.as_b().ok())
                .and_then(|blob| <[u8; 32]>::try_from(blob.as_ref()).ok())?;
            Some(SecretDigest { len, hmac })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use aex_otlp_admission::{MemoryBudget, OtlpError};
    use aex_wire::error::ErrorCode;

    use super::{CustodyManifests, authority_error, otlp_error, reserve};
    use crate::authority::AuthorityError;

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
    fn the_manifest_key_is_the_published_redact_family() {
        use aex_wire::ids::PrefixedId as _;
        let session = aex_wire::ids::SessionId::from_uuid7(aex_wire::Uuid7::compose(1, [4; 10]));
        let key = CustodyManifests::partition_key(session);
        assert!(key.starts_with("REDACT#"), "{key}");
        assert!(key.ends_with(&session.to_string()));
    }
}
