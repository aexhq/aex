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
use aex_wire::idempotency::PrincipalScope;
use aex_wire::ids::{PrefixedId as _, SessionId, TelemetryBatchId, Uuid7, WorkspaceId};
use aex_wire::models::TelemetryAdmissionReceipt;
use aex_wire::routes::route;
use aex_wire::server::{OtlpApi, RequestContext};
use aex_wire::types::{DecimalU128, Timestamp};
use aws_sdk_dynamodb::types::AttributeValue;

use crate::authority::{AdmissionAuthority, AdmissionRequest, AuthorityError, PreparedObservation};
use crate::counters::{AdmissionCounter, AdmissionTelemetry};
use aex_observation_store_dynamodb::spool::GateState;
use aex_regional_http::context::RequestContext as EdgeContext;

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
    telemetry: AdmissionTelemetry,
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
            .field("telemetry", &self.telemetry)
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
        telemetry: AdmissionTelemetry,
    ) -> Self {
        Self {
            authority,
            custody,
            limits,
            budget,
            reserve_wait,
            redaction_key,
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
    /// There is deliberately no session parameter. Everything this request may
    /// be attributed to arrives inside `authorized`, which the edge produced by
    /// verifying a credential; a caller-supplied value can therefore not reach
    /// the attribution at all rather than being checked and hopefully rejected.
    #[must_use]
    pub fn new(
        service: Arc<OtlpService>,
        authorized: EdgeContext,
        encoding: OtlpEncoding,
        coding: ContentCoding,
    ) -> Self {
        Self {
            service,
            authorized,
            encoding,
            coding,
        }
    }

    /// The session the **verified credential** names, when it names one.
    ///
    /// This is the only session source in this deployable, and it is a pure
    /// function of the edge's own output. Ingest is a hot path the owner has
    /// ruled out putting a lookup on, so a session that no credential proves is
    /// not a session this service can attribute to.
    ///
    /// The match is exhaustive **and fully destructured on purpose**: there is
    /// no `_` arm and no `..` rest pattern anywhere in it. A new
    /// [`PrincipalScope`] variant — or a new field on an existing one — that
    /// carries a session therefore fails this build, right here, at the one
    /// place that decides what a batch is attributed to. Whoever introduces a
    /// session-bearing principal cannot land it without answering this
    /// question, and the moment they answer it with `Some`, session attribution
    /// starts working through [`Self::scope`] with no other edit anywhere.
    fn credential_session(&self) -> Option<SessionId> {
        match self.authorized.auth.principal {
            // No regional principal names a session today. A person acting
            // through an account token and a workspace API key are both scoped
            // to a workspace and nothing narrower, so the honest answer for
            // both is "no session" rather than a value taken from the request.
            PrincipalScope::Account {
                user: _,
                organization: _,
            }
            | PrincipalScope::WorkspaceKey {
                key: _,
                workspace: _,
                organization: _,
            } => None,
        }
    }

    /// The scope this request's observations belong to.
    ///
    /// A session-scoped row's partition key (`OBS#S#{session}`) carries no
    /// workspace component, so this value is the whole tenancy decision for
    /// every row the batch writes. It is derived from the credential and from
    /// nothing else.
    #[must_use]
    pub fn scope(&self) -> ScopeKey {
        self.credential_session().map_or(
            ScopeKey::Workspace(self.authorized.auth.workspace_id),
            ScopeKey::Session,
        )
    }

    /// Admits one batch of one signal.
    // This is intentionally kept as one bounded admission transaction so the
    // reservation, decode, normalization, redaction, and commit ordering is
    // visible in one place.
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

    /// Removes AEX-managed secret values the platform itself injected.
    async fn redact(&self, batch: &mut NormalizedBatch) -> WireResult<RedactionReport> {
        let Some(session) = self.scope().session() else {
            // Only a session can have had a platform secret injected into it. A
            // workspace-key batch is customer-authored throughout, and the
            // product never claims arbitrary customer bytes can be recognised.
            //
            // This is also what makes the custody read unreachable while no
            // principal names a session: the manifest `GetItem` below is the
            // only cross-tenant-addressable read on this path, and it can only
            // be reached with a session the credential itself proved.
            return apply(&NoManagedSecrets, batch).map_err(|error| otlp_error(&error));
        };
        let manifest = match self
            .service
            .custody
            .manifest(session, self.authorized.auth.workspace_id)
            .await
        {
            Ok(manifest) => manifest,
            Err(error) => {
                if let AuthorityError::CustodyMalformed { malformed } = &error {
                    // Attempts, not drops: each occurrence is a batch that
                    // failed closed rather than an entry that silently left
                    // the redaction set.
                    self.service
                        .telemetry
                        .count(AdmissionCounter::CustodyEntriesMalformed, *malformed as u64);
                }
                return Err(authority_error(&error));
            }
        };
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
        // Fail closed, retryably: the custody stream owns the manifest row and
        // rewrites it on its next revision, so the same batch can succeed
        // later without any change on the caller's side.
        AuthorityError::CustodyMalformed { .. } => {
            WireError::new(ErrorCode::ObservabilityUnavailable)
                .with_retry_after(Duration::from_secs(30))
        }
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

    /// Reads one session's manifest, refusing one that names another workspace.
    ///
    /// The key is `REDACT#{session}` alone — the item family is partitioned by
    /// session, not by tenant — so the read itself cannot be conditioned on the
    /// caller's workspace. The row does record its owner, though
    /// (`aex_secret_custody_dynamodb::codec::encode_manifest` writes
    /// `workspaceId` on every manifest it emits), so the ownership assertion is
    /// made on the returned item, exactly as that crate's own `decode_manifest`
    /// makes it. Belt and braces: with the session now coming from the verified
    /// credential, the caller can no longer name a session it does not own, and
    /// this refusal is the second, independent reason a foreign manifest can
    /// never be read.
    ///
    /// # Errors
    ///
    /// Returns [`AuthorityError::Provider`] when the read fails,
    /// [`AuthorityError::Malformed`] when the stored manifest is absent an
    /// owner, names a different one, or carries no `entries` attribute, and
    /// [`AuthorityError::CustodyMalformed`] when any entry cannot be parsed.
    /// All three fail the batch closed: admitting unredacted bytes — or bytes
    /// checked against a silently emptier redaction set — is never an
    /// acceptable degradation.
    pub async fn manifest(
        &self,
        session: SessionId,
        workspace: WorkspaceId,
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
            // An absent item means no managed secret was injected into the
            // session. There is no owner to compare, and inventing one would
            // invent a manifest.
            return Ok(SecretDigestManifest {
                session,
                custody_revision: 0,
                entries: Vec::new(),
            });
        };
        owned_by(&item, workspace)?;
        let entries = parse_entries(&item)?;
        Ok(SecretDigestManifest {
            session,
            custody_revision: item
                .get("custodyRevision")
                .and_then(|value| value.as_n().ok())
                .and_then(|text| text.parse().ok())
                .unwrap_or(0),
            entries,
        })
    }
}

/// Asserts a stored manifest belongs to the workspace that is reading it.
///
/// A present row with no `workspaceId` is a refusal, not a pass: the only
/// writer of this family stamps the attribute unconditionally, so an item
/// without one is a row this reader cannot prove the ownership of, and an
/// unprovable owner fails closed.
///
/// # Errors
///
/// Returns [`AuthorityError::Malformed`] when the attribute is absent,
/// unparseable, or names another workspace.
fn owned_by(
    item: &HashMap<String, AttributeValue>,
    workspace: WorkspaceId,
) -> Result<(), AuthorityError> {
    let malformed = AuthorityError::Malformed {
        item: "redaction_manifest",
        attribute: "workspaceId",
    };
    let owner = item
        .get("workspaceId")
        .and_then(|value| value.as_s().ok())
        .ok_or_else(|| malformed.clone())?;
    if owner.parse::<WorkspaceId>().ok() == Some(workspace) {
        Ok(())
    } else {
        Err(malformed)
    }
}

/// Reads the `{len, hmac}` rows of one manifest item.
///
/// The counterpart writer is
/// `aex_secret_custody_dynamodb::codec::encode_manifest`, which emits `entries`
/// as `L` of `M{len: N, hmac: B}`. Nothing in the compiler connects the two —
/// this reader holds the raw `DynamoDB` client — so the shape is pinned on both
/// sides and a test in this module encodes through that writer and parses here.
///
/// An unreadable entry refuses the whole manifest rather than being filtered
/// away: a `filter_map` here silently emptied the redaction set, which is how
/// an injected secret would have reached storage unredacted.
///
/// An **absent** `entries` attribute is the same refusal for the same reason.
/// It used to be read as "a manifest that names no secrets", justified by a
/// custody stream that writes an empty list before the first injection — no
/// such stream exists, and against a malformed or wrong-shaped row that reading
/// admitted telemetry against an empty redaction set. An explicitly present
/// empty list still means no secrets; absence means a row this reader cannot
/// prove the shape of, and an unprovable shape fails closed exactly as an
/// unprovable owner does in [`owned_by`].
///
/// # Errors
///
/// Returns [`AuthorityError::Malformed`] when `entries` is absent or is not a
/// list, and [`AuthorityError::CustodyMalformed`] carrying how many entries
/// could not be parsed.
fn parse_entries(
    item: &HashMap<String, AttributeValue>,
) -> Result<Vec<SecretDigest>, AuthorityError> {
    let list = item
        .get("entries")
        .and_then(|value| value.as_l().ok())
        .ok_or(AuthorityError::Malformed {
            item: "redaction_manifest",
            attribute: "entries",
        })?;
    let mut entries = Vec::with_capacity(list.len());
    let mut malformed = 0usize;
    for entry in list {
        let parsed = entry.as_m().ok().and_then(|map| {
            let len = map
                .get("len")
                .and_then(|value| value.as_n().ok())
                .and_then(|text| text.parse::<u16>().ok())?;
            let hmac = map
                .get("hmac")
                .and_then(|value| value.as_b().ok())
                .and_then(|blob| <[u8; 32]>::try_from(blob.as_ref()).ok())?;
            Some(SecretDigest { len, hmac })
        });
        match parsed {
            Some(digest) => entries.push(digest),
            None => malformed += 1,
        }
    }
    if malformed > 0 {
        return Err(AuthorityError::CustodyMalformed { malformed });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use aex_observation_domain::canonical::{BatchBinding, CanonicalValue, batch_intent_digest};
    use aex_observation_domain::keys::ScopeKey;
    use aex_otlp_admission::{
        ContentCoding, MemoryBudget, NormalizedBatch, OtlpEncoding, OtlpError, OtlpLimits,
        OverwriteReport,
    };
    use aex_wire::error::ErrorCode;
    use aex_wire::idempotency::PrincipalScope;
    use aex_wire::ids::{ApiKeyId, OrganizationId, PrefixedId as _, SessionId, WorkspaceId};

    use super::{
        CustodyManifests, OtlpRequest, OtlpService, authority_error, batch_id_for, otlp_error,
        owned_by, reserve,
    };
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

    /// One manifest item whose entry list is exactly `entries`.
    fn manifest_item(
        entries: Vec<aws_sdk_dynamodb::types::AttributeValue>,
    ) -> std::collections::HashMap<String, aws_sdk_dynamodb::types::AttributeValue> {
        use aws_sdk_dynamodb::types::AttributeValue;
        std::collections::HashMap::from([
            ("custodyRevision".to_owned(), AttributeValue::N("3".into())),
            ("entries".to_owned(), AttributeValue::L(entries)),
        ])
    }

    /// One well-formed `{len, hmac}` entry.
    fn custody_entry() -> aws_sdk_dynamodb::types::AttributeValue {
        use aws_sdk_dynamodb::types::AttributeValue;
        AttributeValue::M(std::collections::HashMap::from([
            ("len".to_owned(), AttributeValue::N("12".into())),
            (
                "hmac".to_owned(),
                AttributeValue::B(aws_sdk_dynamodb::primitives::Blob::new(vec![7u8; 32])),
            ),
        ]))
    }

    #[test]
    fn a_malformed_custody_entry_refuses_the_manifest_rather_than_shrinking_it() {
        use aws_sdk_dynamodb::types::AttributeValue;
        // The pre-fix `filter_map` dropped what it could not parse, so a
        // corrupt entry silently emptied the redaction set and an injected
        // secret would have reached storage unredacted.
        let truncated_hmac = AttributeValue::M(std::collections::HashMap::from([
            ("len".to_owned(), AttributeValue::N("12".into())),
            (
                "hmac".to_owned(),
                AttributeValue::B(aws_sdk_dynamodb::primitives::Blob::new(vec![7u8; 16])),
            ),
        ]));
        let missing_len = AttributeValue::M(std::collections::HashMap::from([(
            "hmac".to_owned(),
            AttributeValue::B(aws_sdk_dynamodb::primitives::Blob::new(vec![7u8; 32])),
        )]));
        let outcome = super::parse_entries(&manifest_item(vec![
            custody_entry(),
            truncated_hmac,
            missing_len,
        ]));
        assert!(
            matches!(
                outcome.expect_err("two unreadable entries refuse the manifest"),
                AuthorityError::CustodyMalformed { malformed: 2 }
            ),
            "every attempt is counted, not only the first"
        );
    }

    #[test]
    fn a_well_formed_manifest_parses_whole() {
        let entries =
            super::parse_entries(&manifest_item(vec![custody_entry()])).expect("parses whole");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].len, 12);
        assert_eq!(entries[0].hmac, [7u8; 32]);
    }

    #[test]
    fn an_explicitly_empty_entry_list_is_a_session_that_names_no_secrets() {
        let empty = super::parse_entries(&manifest_item(Vec::new()))
            .expect("an explicit empty list names no secrets");
        assert!(empty.is_empty());
    }

    #[test]
    fn a_manifest_row_with_no_entry_list_is_refused_rather_than_read_as_empty() {
        // Absent is not empty. This branch used to return an empty redaction
        // set, justified by a custody stream that writes the row with an empty
        // list before the first injection; no writer does that, so against a
        // malformed or wrong-shaped row the justification admitted telemetry
        // unredacted — the exact failure the neighbouring entry-parse refusal
        // exists to prevent.
        let error = super::parse_entries(&std::collections::HashMap::from([(
            "custodyRevision".to_owned(),
            aws_sdk_dynamodb::types::AttributeValue::N("3".into()),
        )]))
        .expect_err("an absent attribute is a row this reader cannot prove");
        assert!(
            matches!(
                error,
                AuthorityError::Malformed {
                    item: "redaction_manifest",
                    attribute: "entries"
                }
            ),
            "{error}"
        );
        assert_eq!(
            authority_error(&error).code,
            ErrorCode::InternalError,
            "a wrong-shaped custody row fails the batch closed"
        );
    }

    #[test]
    fn an_entry_list_of_the_wrong_type_is_refused() {
        let error = super::parse_entries(&std::collections::HashMap::from([(
            "entries".to_owned(),
            aws_sdk_dynamodb::types::AttributeValue::S("[]".to_owned()),
        )]))
        .expect_err("a scalar is not an entry list");
        assert!(matches!(error, AuthorityError::Malformed { .. }), "{error}");
    }

    /// The writer and the reader of the manifest agree, byte for byte.
    ///
    /// There is no compiler edge between
    /// `aex_secret_custody_dynamodb::codec::encode_manifest` and
    /// [`super::parse_entries`] — this service reaches the row through the raw
    /// `DynamoDB` client — and the two shapes had in fact diverged: the writer
    /// published a flat list of hex digests with no length, which this
    /// sliding-window reader cannot use at all. The dev-dependency exists so
    /// that divergence is a failing test rather than an empty redaction set.
    #[test]
    fn what_the_custody_writer_encodes_is_exactly_what_this_reader_parses() {
        use aex_observation_domain::canonical::CanonicalValue;
        use aex_otlp_admission::redact::REDACTED;
        use aex_otlp_admission::{
            DigestRedactor, ManagedSecretRedactor as _, SecretDigestManifest,
        };
        use aex_secret_custody_dynamodb::codec::{
            RedactionEntry, RedactionManifest, encode_manifest,
        };

        const KEY: &[u8] = b"regional-redaction-key";
        const SECRET: &str = "sk-live-abcdef";

        let session: SessionId =
            aex_wire::ids::PrefixedId::parse("ses_0000000003ec1r60r30c1g60r3").expect("a session");
        let written = RedactionManifest {
            session,
            workspace: WorkspaceId::from_uuid7(aex_wire::Uuid7::compose(1, [1; 10])),
            revision: aex_secret_domain::custody::CustodyRevision::FIRST,
            algorithm: "HMAC-SHA-256".to_owned(),
            key_id: "redact-key-1".to_owned(),
            entries: vec![RedactionEntry::digest(KEY, SECRET.as_bytes()).expect("a narrow secret")],
            updated_at: aex_wire::types::Timestamp::parse("2026-08-09T00:00:00.000Z")
                .expect("a timestamp"),
        };

        let parsed = super::parse_entries(&encode_manifest(&written))
            .expect("the reader parses what the writer wrote");
        assert_eq!(parsed.len(), 1);
        assert_eq!(
            usize::from(parsed[0].len),
            SECRET.len(),
            "the declared length survives the row; without it there is no window to slide"
        );

        // End to end: a digest produced by the writer matches through the
        // reader's own matcher, so the two HMAC constructions are one contract.
        let redactor = DigestRedactor::new(
            KEY.to_vec(),
            SecretDigestManifest {
                session,
                custody_revision: 1,
                entries: parsed,
            },
            u64::MAX,
        );
        let mut value = CanonicalValue::Str(format!("token is {SECRET} ok").into());
        assert_eq!(
            redactor
                .redact(&mut value)
                .expect("redacts")
                .redacted_values,
            1
        );
        assert_eq!(value.as_str(), Some(REDACTED));
    }

    #[test]
    fn a_malformed_custody_manifest_fails_the_batch_closed_and_retryably() {
        let error = AuthorityError::CustodyMalformed { malformed: 2 };
        assert!(
            error.retryable(),
            "the custody stream rewrites the row; the caller retries unchanged"
        );
        let wire = authority_error(&error);
        assert_eq!(wire.code, ErrorCode::ObservabilityUnavailable);
        assert!(wire.retry_after.is_some(), "fail closed, never fail open");
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

    #[test]
    fn the_manifest_key_is_the_published_redact_family() {
        let session = aex_wire::ids::SessionId::from_uuid7(aex_wire::Uuid7::compose(1, [4; 10]));
        let key = CustodyManifests::partition_key(session);
        assert!(key.starts_with("REDACT#"), "{key}");
        assert!(key.ends_with(&session.to_string()));
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

    /// A workspace nobody in these cases is authenticated for.
    fn other_workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(aex_wire::Uuid7::compose(1, [3; 10]))
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

    /// One request whose custody client would fail loudly if it were used.
    ///
    /// The replay client is handed **no** events, so any provider call at all
    /// is an error rather than a silently satisfied read. That is what makes
    /// "the custody read is unreachable" a measured fact here rather than a
    /// claim about the source.
    fn request_with_unusable_provider() -> (
        OtlpRequest,
        aws_smithy_http_client::test_util::StaticReplayClient,
    ) {
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
                dynamodb.clone(),
                s3,
                "test-observation-authority",
                "test-observation-bucket",
                region,
            ),
            CustodyManifests::new(dynamodb, "test-secret-custody"),
            OtlpLimits::REGISTERED,
            MemoryBudget::new(1024 * 1024),
            Duration::from_millis(5),
            vec![0u8; 32],
            AdmissionTelemetry::new(
                aex_platform_telemetry::Handle::install(
                    &aex_platform_telemetry::Settings::default(),
                    None,
                ),
                "dev",
                "eu-west-1",
            ),
        );
        (
            OtlpRequest::new(
                Arc::new(service),
                authorized(),
                OtlpEncoding::Protobuf,
                ContentCoding::Identity,
            ),
            replay,
        )
    }

    #[test]
    fn a_batch_is_attributed_to_the_authenticated_workspace_and_to_nothing_a_caller_can_send() {
        // `OtlpRequest` has no session input of any kind, so the only fixture a
        // caller-controlled attribution could be built from does not exist.
        // What remains is the derivation, and it must read the credential.
        let (request, _replay) = request_with_unusable_provider();
        assert_eq!(
            request.scope(),
            ScopeKey::Workspace(workspace()),
            "the batch is bound to the workspace the edge verified"
        );
        assert_eq!(
            request.scope().session(),
            None,
            "no regional principal names a session, so no batch may claim one"
        );
        assert_ne!(
            request.scope(),
            ScopeKey::Session(session()),
            "a session scope is unreachable while no credential proves a session"
        );
    }

    #[test]
    fn the_stored_partition_key_of_an_admitted_batch_carries_the_workspace() {
        // A session-scoped row's key is `OBS#S#{session}` and holds no
        // workspace, which is exactly why the scope may not come from the
        // request. With the credential as the only source, every row this
        // deployable writes today is addressable only through its tenant.
        let (request, _replay) = request_with_unusable_provider();
        let key = request.scope().to_key();
        assert_eq!(key, format!("W#{}", workspace()));
        assert!(
            !key.starts_with("S#"),
            "a workspace-key batch may never land in a session partition"
        );
    }

    #[tokio::test]
    async fn the_custody_manifest_read_is_unreachable_without_a_credential_named_session() {
        // `redact` short-circuits on "no session", so the unconditioned
        // `GetItem` on `REDACT#{session}` — the one cross-tenant addressable
        // read left on this path — is never issued. The provider would fail if
        // it were.
        let (request, replay) = request_with_unusable_provider();
        let mut batch = NormalizedBatch {
            observations: Vec::new(),
            report: OverwriteReport::default(),
        };
        let report = request
            .redact(&mut batch)
            .await
            .expect("a workspace batch redacts against the empty managed set");
        assert_eq!(report.redacted_values, 0);
        assert_eq!(
            replay.actual_requests().count(),
            0,
            "ingest must issue no custody read at all"
        );
    }

    #[test]
    fn a_custody_manifest_naming_another_workspace_is_refused() {
        use aws_sdk_dynamodb::types::AttributeValue;
        let mut item = manifest_item(vec![custody_entry()]);
        item.insert(
            "workspaceId".to_owned(),
            AttributeValue::S(other_workspace().to_string()),
        );
        let error = owned_by(&item, workspace()).expect_err("a foreign manifest is refused");
        assert!(
            matches!(
                error,
                AuthorityError::Malformed {
                    item: "redaction_manifest",
                    attribute: "workspaceId"
                }
            ),
            "{error:?}"
        );
        assert!(
            !error.retryable(),
            "a manifest owned by another tenant never becomes readable by retrying"
        );

        item.insert(
            "workspaceId".to_owned(),
            AttributeValue::S(workspace().to_string()),
        );
        owned_by(&item, workspace()).expect("the owner's own manifest reads");
    }

    #[test]
    fn a_custody_manifest_that_cannot_prove_its_owner_is_refused() {
        // The only writer of this family stamps `workspaceId` unconditionally,
        // so a present row without one is a row whose ownership this reader
        // cannot establish. It fails closed rather than being read.
        let error = owned_by(&manifest_item(vec![custody_entry()]), workspace())
            .expect_err("an unattributed manifest is refused");
        assert!(
            matches!(
                error,
                AuthorityError::Malformed {
                    item: "redaction_manifest",
                    attribute: "workspaceId"
                }
            ),
            "{error:?}"
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
