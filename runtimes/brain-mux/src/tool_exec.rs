//! Brain's sole client for the private platform-paid tool executor.

use std::sync::Arc;

use aex_brain_app::ports::{
    BoxFuture, CancelToken, DetachedStatus, DispatchTicket, PreparedToolCall, ProviderFailureKind,
    RedactedDetail, ToolDispatchError, ToolOutcome, ToolResultBody,
};
use aex_brain_domain::effect::{DispatchProof, DispatchStage};
use aex_brain_domain::ids::{ContentHash, DetachedOperationId, Fence};
use aex_brain_domain::journal::ExecutorRoute;
use aex_brain_domain::wire_pending::ToolResultPart;
use aex_brain_tool_catalog::router::ToolExecutor;
use aex_control_domain::scope::ScopeSet;
use aex_identity_domain::assertion::{
    ASSERTION_MAX_LIFETIME_MS, AssertedAccountState, AssertionClaims, Audience, EpochSlots, KeyId,
    LocalSigner, Plane, PrincipalKind, agent_session_binding, issue,
};
use aex_internal_contracts::SchemaVersion;
use aex_internal_contracts::assertion::AssertionAudience;
use aex_internal_contracts::tool_exec::{
    ArgumentsJcs, DeadlineMs, EffectRef, ToolExecRefusal, ToolExecRequest, ToolExecResponse,
    ToolResultPart as WireToolResultPart,
};
use aex_model_catalog::BoundedString;
use aex_wire::ids::{ContentHash as WireContentHash, PrefixedId as _, ResourceName};
use aex_wire::types::Region;
use base64::Engine as _;
use futures::StreamExt as _;
use zeroize::Zeroizing;

const WEB_SEARCH: &str = "web_search";
const RESPONSE_OVERHEAD_BYTES: usize = 8_192;

/// Non-secret deployment settings plus the one secret reference resolved at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// Private Cloud Map HTTP route.
    pub endpoint: String,
    /// Secrets Manager id holding 32 raw Ed25519 bytes as unpadded base64url.
    pub signing_secret_id: String,
    /// Key identity published to the executor.
    pub signing_key_id: uuid::Uuid,
    /// Matching public key, for the cold-start self-test.
    pub signing_public_key: [u8; 32],
    /// Deployment plane.
    pub plane: Plane,
    /// Deployment region.
    pub region: Region,
}

/// Why the production executor binding refused startup.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BindingError {
    /// Endpoint configuration was not the private one-route shape.
    #[error("the tool-executor endpoint is invalid")]
    Endpoint,
    /// The signing secret could not be read.
    #[error("the tool-exec signing secret is unavailable")]
    SecretUnavailable,
    /// The signing secret was not the canonical 32-byte representation.
    #[error("the tool-exec signing secret is malformed")]
    SecretMalformed,
    /// The secret did not match the published verifier.
    #[error("the tool-exec signing secret does not match its public key")]
    SigningKeyMismatch,
    /// The compiled catalog identities could not be bound.
    #[error("the tool-exec catalog identity is invalid")]
    Catalog,
}

/// Loads the local signer once and returns the production executor.
///
/// # Errors
///
/// Refuses an invalid private endpoint, unavailable or malformed signer,
/// signer/verifier mismatch, or invalid compiled catalog identity.
pub async fn production(
    aws: &aws_config::SdkConfig,
    binding: Binding,
) -> Result<Arc<dyn ToolExecutor>, BindingError> {
    validate_endpoint(&binding.endpoint)?;
    let secret = aws_sdk_secretsmanager::Client::new(aws)
        .get_secret_value()
        .secret_id(&binding.signing_secret_id)
        .send()
        .await
        .map_err(|_| BindingError::SecretUnavailable)?;
    let document = secret
        .secret_string()
        .ok_or(BindingError::SecretMalformed)?;
    let material = parse_signing_secret(document)?;
    let signer = LocalSigner::new(KeyId::new(binding.signing_key_id), &material);
    if signer.public_key() != binding.signing_public_key {
        return Err(BindingError::SigningKeyMismatch);
    }
    let transport = Arc::new(HttpTransport::new(&binding.endpoint)?);
    let certified_manifest =
        WireContentHash::parse(aex_brain_tool_catalog::catalog::BUILTIN_CATALOG_DIGEST)
            .map_err(|_| BindingError::Catalog)?;
    let route_manifest = ContentHash::of(
        &aex_brain_tool_catalog::catalog::builtin_catalog_bytes()
            .map_err(|_| BindingError::Catalog)?,
    );
    Ok(Arc::new(ToolExecExecutor::new(
        transport,
        Arc::new(signer),
        binding.plane,
        binding.region,
        certified_manifest,
        route_manifest,
    )))
}

fn parse_signing_secret(document: &str) -> Result<Zeroizing<[u8; 32]>, BindingError> {
    if document.len() > 1_024 || document.trim() != document {
        return Err(BindingError::SecretMalformed);
    }
    let bytes = Zeroizing::new(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(document)
            .map_err(|_| BindingError::SecretMalformed)?,
    );
    let material =
        <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| BindingError::SecretMalformed)?;
    if base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(material) != document {
        return Err(BindingError::SecretMalformed);
    }
    Ok(Zeroizing::new(material))
}

fn validate_endpoint(endpoint: &str) -> Result<(), BindingError> {
    let url = reqwest::Url::parse(endpoint).map_err(|_| BindingError::Endpoint)?;
    let valid = url.scheme() == "http"
        && url
            .host_str()
            .is_some_and(|host| host.ends_with(".internal"))
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && url.path() == "/internal/tool-exec";
    if valid {
        Ok(())
    } else {
        Err(BindingError::Endpoint)
    }
}

trait Transport: Send + Sync + 'static {
    fn execute<'a>(
        &'a self,
        request: &'a ToolExecRequest,
        max_result_bytes: usize,
    ) -> BoxFuture<'a, Result<ToolExecResponse, ToolExecTransportError>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolExecTransportError {
    NotSent,
    PossiblySent,
    Response,
}

struct HttpTransport {
    client: reqwest::Client,
    endpoint: reqwest::Url,
}

impl HttpTransport {
    fn new(endpoint: &str) -> Result<Self, BindingError> {
        validate_endpoint(endpoint)?;
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| BindingError::Endpoint)?,
            endpoint: reqwest::Url::parse(endpoint).map_err(|_| BindingError::Endpoint)?,
        })
    }
}

impl Transport for HttpTransport {
    fn execute<'a>(
        &'a self,
        request: &'a ToolExecRequest,
        max_result_bytes: usize,
    ) -> BoxFuture<'a, Result<ToolExecResponse, ToolExecTransportError>> {
        Box::pin(async move {
            let outbound = self
                .client
                .post(self.endpoint.clone())
                .timeout(core::time::Duration::from_millis(u64::from(
                    request.deadline_ms.get(),
                )))
                .json(request)
                .build()
                .map_err(|_| ToolExecTransportError::NotSent)?;
            let response = self
                .client
                .execute(outbound)
                .await
                .map_err(|_| ToolExecTransportError::PossiblySent)?;
            if response.status() != reqwest::StatusCode::OK {
                return Err(ToolExecTransportError::Response);
            }
            let limit = max_result_bytes.saturating_add(RESPONSE_OVERHEAD_BYTES);
            if response
                .content_length()
                .is_some_and(|length| usize::try_from(length).map_or(true, |length| length > limit))
            {
                return Err(ToolExecTransportError::Response);
            }
            let mut body = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| ToolExecTransportError::Response)?;
                if body.len().saturating_add(chunk.len()) > limit {
                    return Err(ToolExecTransportError::Response);
                }
                body.extend_from_slice(&chunk);
            }
            serde_json::from_slice(&body).map_err(|_| ToolExecTransportError::Response)
        })
    }
}

/// Exact `ToolExecutor` implementation for platform-paid search.
struct ToolExecExecutor {
    transport: Arc<dyn Transport>,
    signer: Arc<LocalSigner>,
    plane: Plane,
    region: Region,
    certified_manifest: WireContentHash,
    route_manifest: ContentHash,
}

impl ToolExecExecutor {
    fn new(
        transport: Arc<dyn Transport>,
        signer: Arc<LocalSigner>,
        plane: Plane,
        region: Region,
        certified_manifest: WireContentHash,
        route_manifest: ContentHash,
    ) -> Self {
        Self {
            transport,
            signer,
            plane,
            region,
            certified_manifest,
            route_manifest,
        }
    }

    fn request(
        &self,
        ticket: &DispatchTicket,
        call: &PreparedToolCall,
    ) -> Result<ToolExecRequest, ToolDispatchError> {
        if call.route.executor != ExecutorRoute::ToolExec
            || !self.supports(&call.route.name)
            || call.route.manifest_digest != self.route_manifest
        {
            return Err(pre_dispatch(
                "tool-exec received a call outside its closed route",
            ));
        }
        let issued_at_ms = u64::try_from(ticket.issued_at().millis())
            .map_err(|_| pre_dispatch("tool dispatch ticket has an invalid timestamp"))?;
        let lifetime_ms = u64::from(call.route.timeout_ms).min(ASSERTION_MAX_LIFETIME_MS);
        let expires_at_ms = issued_at_ms
            .checked_add(lifetime_ms)
            .ok_or_else(|| pre_dispatch("tool assertion timestamp overflow"))?;
        let assertion = issue(
            self.signer.as_ref(),
            &AssertionClaims {
                issued_at_ms,
                expires_at_ms,
                audience: Audience {
                    plane: self.plane,
                    region: self.region,
                    service: AssertionAudience::ToolExec,
                },
                principal_kind: PrincipalKind::AgentSession,
                principal_id: ticket.key().session.0,
                credential_binding: agent_session_binding(
                    uuid::Uuid::from_bytes(ticket.effect().0),
                    ticket.attempt(),
                ),
                organization_id: uuid::Uuid::from_bytes(*ticket.organization().uuid7().as_bytes()),
                workspace_id: uuid::Uuid::from_bytes(*ticket.workspace().uuid7().as_bytes()),
                workspace_region: self.region,
                // A DispatchTicket exists only inside an admitted, lease-owning activation.
                // Account-pause propagation revokes that authority; the locally signed claim
                // states the authority the Brain currently holds rather than re-reading central.
                account_state: AssertedAccountState::Active,
                scopes: ScopeSet::EMPTY,
                epochs: EpochSlots::EMPTY,
            },
        )
        .map_err(|_| pre_dispatch("tool assertion could not be issued"))?;
        Ok(ToolExecRequest {
            schema_version: SchemaVersion::V1,
            assertion: assertion
                .to_issued()
                .map_err(|_| pre_dispatch("tool assertion could not be encoded"))?,
            tool: ResourceName::parse(call.route.name.as_str())
                .map_err(|_| pre_dispatch("tool name is outside the internal grammar"))?,
            manifest: self.certified_manifest,
            arguments_jcs: ArgumentsJcs::new(call.input.as_bytes().to_vec())
                .map_err(|_| pre_dispatch("tool arguments exceed the internal bound"))?,
            effect: EffectRef::new(ticket.effect().0),
            attempt: ticket.attempt(),
            deadline_ms: DeadlineMs::new(call.route.timeout_ms)
                .map_err(|_| pre_dispatch("tool deadline exceeds the internal bound"))?,
        })
    }
}

impl core::fmt::Debug for ToolExecExecutor {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ToolExecExecutor")
            .finish_non_exhaustive()
    }
}

impl ToolExecutor for ToolExecExecutor {
    fn supports(&self, tool: &aex_brain_domain::ids::ToolName) -> bool {
        tool.as_str() == WEB_SEARCH
    }

    fn invoke<'a>(
        &'a self,
        ticket: &'a DispatchTicket,
        call: &'a PreparedToolCall,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<ToolOutcome, ToolDispatchError>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(cancelled());
            }
            let request = self.request(ticket, call)?;
            match self
                .transport
                .execute(&request, call.max_result_bytes)
                .await
            {
                Ok(ToolExecResponse::Completed {
                    schema_version: SchemaVersion::V1,
                    content,
                    is_error,
                    duration_ms,
                    checksum,
                }) => {
                    let wire_content = serde_json::to_vec(&content)
                        .map_err(|_| response_error("tool result could not be encoded"))?;
                    let observed_checksum = ContentHash::of(&wire_content);
                    if observed_checksum.0 != *checksum.as_bytes() {
                        return Err(response_error(
                            "tool result checksum did not match its body",
                        ));
                    }
                    let content = content
                        .into_iter()
                        .map(|part| match part {
                            WireToolResultPart::Text { text } => {
                                BoundedString::new(text).map(|text| ToolResultPart::Text { text })
                            }
                        })
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|_| response_error("tool result exceeds the domain bound"))?;
                    if wire_content.len() > call.max_result_bytes {
                        return Err(response_error(
                            "tool result exceeds the exact catalog bound",
                        ));
                    }
                    Ok(ToolOutcome::Completed(ToolResultBody {
                        content,
                        is_error,
                        duration_ms,
                        executed_on: ExecutorRoute::ToolExec,
                        checksum: observed_checksum,
                    }))
                }
                Ok(ToolExecResponse::Refused {
                    schema_version: SchemaVersion::V1,
                    reason,
                }) => Err(refusal(reason)),
                Ok(ToolExecResponse::Completed { .. } | ToolExecResponse::Refused { .. }) => Err(
                    response_error("tool-exec returned an unsupported schema version"),
                ),
                Err(error) => Err(transport_error(error)),
            }
        })
    }

    fn query<'a>(
        &'a self,
        _operation: &'a DetachedOperationId,
    ) -> BoxFuture<'a, Result<DetachedStatus, ToolDispatchError>> {
        Box::pin(async { Err(pre_dispatch("tool-exec calls never detach")) })
    }

    fn cancel<'a>(
        &'a self,
        _operation: &'a DetachedOperationId,
        _fence: Fence,
    ) -> BoxFuture<'a, Result<(), ToolDispatchError>> {
        Box::pin(async { Err(pre_dispatch("tool-exec calls have no detached operation")) })
    }
}

fn pre_dispatch(message: &str) -> ToolDispatchError {
    failure(
        DispatchStage::PreDispatch,
        DispatchProof::NotSent,
        false,
        ProviderFailureKind::InvalidRequest,
        message,
    )
}

fn cancelled() -> ToolDispatchError {
    failure(
        DispatchStage::PreDispatch,
        DispatchProof::NotSent,
        false,
        ProviderFailureKind::Cancelled,
        "tool-exec call cancelled before dispatch",
    )
}

fn response_error(message: &str) -> ToolDispatchError {
    failure(
        DispatchStage::Streaming,
        DispatchProof::ResponseStarted,
        false,
        ProviderFailureKind::ProtocolViolation,
        message,
    )
}

fn refusal(reason: ToolExecRefusal) -> ToolDispatchError {
    let (kind, message) = match reason {
        ToolExecRefusal::NotAuthorized => (
            ProviderFailureKind::Authentication,
            "tool-exec did not authorize the call",
        ),
        ToolExecRefusal::LimitExceeded => (
            ProviderFailureKind::Quota,
            "the platform tool ceiling was reached",
        ),
        ToolExecRefusal::Unsupported => (
            ProviderFailureKind::InvalidRequest,
            "tool-exec refused the tool or manifest",
        ),
    };
    failure(
        DispatchStage::Terminal,
        DispatchProof::ResponseStarted,
        false,
        kind,
        message,
    )
}

fn transport_error(error: ToolExecTransportError) -> ToolDispatchError {
    match error {
        ToolExecTransportError::NotSent => failure(
            DispatchStage::PreDispatch,
            DispatchProof::NotSent,
            true,
            ProviderFailureKind::Transport,
            "tool-exec request could not be constructed",
        ),
        ToolExecTransportError::PossiblySent => failure(
            DispatchStage::Dispatched,
            DispatchProof::PossiblySent,
            false,
            ProviderFailureKind::Transport,
            "tool-exec transport failed after dispatch began",
        ),
        ToolExecTransportError::Response => {
            response_error("tool-exec returned an invalid response")
        }
    }
}

fn failure(
    stage: DispatchStage,
    proof: DispatchProof,
    retryable: bool,
    kind: ProviderFailureKind,
    message: &str,
) -> ToolDispatchError {
    ToolDispatchError {
        stage,
        proof,
        retryable,
        detail: RedactedDetail::internal(kind, message),
    }
}

#[cfg(test)]
mod tests;
