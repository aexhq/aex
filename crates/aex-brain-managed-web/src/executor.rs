//! Production `ToolExecutor` for the two Brain-managed web tools.
//!
//! The executor owns only call decoding and the dispatch-proof boundary. URL,
//! DNS, response, decompression, media and timeout policy remain in the fetch
//! and search adapters. Search credentials are resolved from the complete
//! dispatch ticket; no tenant credential is retained in the mux.

use std::sync::Arc;
use std::time::Instant;

use aex_brain_application::ports::{
    BoxFuture, CancelToken, DetachedStatus, DispatchTicket, PreparedToolCall, ProviderFailureKind,
    RedactedDetail, ToolDispatchError, ToolOutcome, ToolResultBody,
};
use aex_brain_domain::effect::{DispatchProof, DispatchStage};
use aex_brain_domain::ids::{ContentHash, DetachedOperationId, Fence};
use aex_brain_domain::journal::ExecutorRoute;
use aex_brain_tool_catalog::router::ToolExecutor;
use aex_model_catalog::canonical::ToolResultPart;
use aex_wire::CanonicalJson;
use serde::Deserialize;

use crate::egress::{DnsResolver, SystemDnsResolver};
use crate::fetch::{FetchDocument, FetchFormat, FetchRequest, fetch};
use crate::search::{
    SearchFreshness, WebSearchCredential, WebSearchRequest, WebSearchResult, search,
};

/// Resolves the one optional managed-search credential for the current request.
///
/// Implementations must authorize against the exact session custody revision
/// before decrypt and must not return a credential from another tenant. The
/// complete ticket supplies session, workspace, organization, effect, attempt,
/// and time without any process-global fallback.
pub trait WebSearchCredentialSource: Send + Sync + 'static {
    /// Resolves and parses the current session's admitted credential.
    fn resolve<'a>(
        &'a self,
        ticket: &'a DispatchTicket,
    ) -> BoxFuture<'a, Result<WebSearchCredential, CredentialSourceError>>;
}

/// Why workspace search authority could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CredentialSourceError {
    /// The session has not admitted managed search.
    #[error("session has no admitted managed-search credential")]
    Missing,
    /// Custody was temporarily unavailable.
    #[error("managed-search credential custody is unavailable")]
    Unavailable,
    /// The stored credential did not match the closed provider/key schema.
    #[error("managed-search credential is malformed")]
    Malformed,
}

/// Network operations used by [`ManagedWebExecutor`].
///
/// The production implementation below calls the real bounded adapters. This
/// seam keeps executor contract tests hermetic without replacing DNS or HTTP
/// inside the adapters' own conformance suites.
pub trait ManagedWebClient: Send + Sync + 'static {
    /// Executes one bounded fetch.
    fn fetch<'a>(
        &'a self,
        request: FetchRequest<'a>,
    ) -> BoxFuture<'a, Result<FetchDocument, ManagedWebCallError>>;

    /// Executes one bounded search.
    fn search<'a>(
        &'a self,
        request: WebSearchRequest<'a>,
        credential: &'a WebSearchCredential,
    ) -> BoxFuture<'a, Result<WebSearchResult, ManagedWebCallError>>;
}

/// The real managed-web network client.
#[derive(Clone)]
pub struct ProductionManagedWebClient {
    resolver: Arc<dyn DnsResolver>,
}

impl ProductionManagedWebClient {
    /// Uses the system resolver and the adapter's complete-set screening.
    #[must_use]
    pub fn new() -> Self {
        Self {
            resolver: Arc::new(SystemDnsResolver),
        }
    }

    /// Binds an explicit resolver, primarily for deterministic integration
    /// environments that still exercise the real HTTP path.
    #[must_use]
    pub fn with_resolver(resolver: Arc<dyn DnsResolver>) -> Self {
        Self { resolver }
    }
}

impl Default for ProductionManagedWebClient {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for ProductionManagedWebClient {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ProductionManagedWebClient")
            .finish_non_exhaustive()
    }
}

impl ManagedWebClient for ProductionManagedWebClient {
    fn fetch<'a>(
        &'a self,
        request: FetchRequest<'a>,
    ) -> BoxFuture<'a, Result<FetchDocument, ManagedWebCallError>> {
        Box::pin(async move {
            fetch(request, self.resolver.as_ref())
                .await
                .map_err(|error| ManagedWebCallError::Fetch(error.to_string()))
        })
    }

    fn search<'a>(
        &'a self,
        request: WebSearchRequest<'a>,
        credential: &'a WebSearchCredential,
    ) -> BoxFuture<'a, Result<WebSearchResult, ManagedWebCallError>> {
        Box::pin(async move {
            search(request, credential, self.resolver.as_ref())
                .await
                .map_err(|error| ManagedWebCallError::Search(error.to_string()))
        })
    }
}

/// A redacted managed-web call failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManagedWebCallError {
    /// Fetch failed after entering its bounded network adapter.
    #[error("managed web fetch failed: {0}")]
    Fetch(String),
    /// Search failed after entering its bounded network adapter.
    #[error("managed web search failed: {0}")]
    Search(String),
}

/// Concrete executor for `web_fetch` and `web_search`.
pub struct ManagedWebExecutor {
    client: Arc<dyn ManagedWebClient>,
    credentials: Arc<dyn WebSearchCredentialSource>,
}

impl ManagedWebExecutor {
    /// Binds the real bounded network adapter and request-scoped credential source.
    #[must_use]
    pub fn production(credentials: Arc<dyn WebSearchCredentialSource>) -> Self {
        Self::new(Arc::new(ProductionManagedWebClient::new()), credentials)
    }

    /// Binds explicit peers.
    #[must_use]
    pub fn new(
        client: Arc<dyn ManagedWebClient>,
        credentials: Arc<dyn WebSearchCredentialSource>,
    ) -> Self {
        Self {
            client,
            credentials,
        }
    }
}

impl core::fmt::Debug for ManagedWebExecutor {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ManagedWebExecutor")
            .finish_non_exhaustive()
    }
}

impl ToolExecutor for ManagedWebExecutor {
    fn supports(&self, tool: &aex_brain_domain::ids::ToolName) -> bool {
        matches!(tool.as_str(), "web_fetch" | "web_search")
    }

    fn invoke<'a>(
        &'a self,
        ticket: &'a DispatchTicket,
        call: &'a PreparedToolCall,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<ToolOutcome, ToolDispatchError>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(dispatch_error(
                    DispatchStage::PreDispatch,
                    DispatchProof::NotSent,
                    ProviderFailureKind::Cancelled,
                    "managed web call was cancelled before dispatch",
                ));
            }
            let started = Instant::now();
            let value = match call.route.name.as_str() {
                "web_fetch" => execute_fetch(self.client.as_ref(), &call.input).await?,
                "web_search" => {
                    let credential = self
                        .credentials
                        .resolve(ticket)
                        .await
                        .map_err(|error| credential_error(&error))?;
                    execute_search(self.client.as_ref(), &call.input, &credential).await?
                }
                _ => {
                    return Err(dispatch_error(
                        DispatchStage::PreDispatch,
                        DispatchProof::NotSent,
                        ProviderFailureKind::InvalidRequest,
                        "managed web executor received a tool outside its closed route",
                    ));
                }
            };
            let canonical = CanonicalJson::from_value(&value).map_err(|_| {
                dispatch_error(
                    DispatchStage::Terminal,
                    DispatchProof::ResponseStarted,
                    ProviderFailureKind::ProtocolViolation,
                    "managed web result could not be canonicalized",
                )
            })?;
            if canonical.as_bytes().len() > call.max_result_bytes {
                return Err(dispatch_error(
                    DispatchStage::Terminal,
                    DispatchProof::ResponseStarted,
                    ProviderFailureKind::InvalidRequest,
                    "managed web result exceeds the caller's exact result bound",
                ));
            }
            let checksum = ContentHash::of(canonical.as_bytes());
            Ok(ToolOutcome::Completed(ToolResultBody {
                content: vec![ToolResultPart::Json { value: canonical }],
                is_error: false,
                duration_ms: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
                executed_on: ExecutorRoute::ManagedWeb,
                checksum,
            }))
        })
    }

    fn query<'a>(
        &'a self,
        _operation: &'a DetachedOperationId,
    ) -> BoxFuture<'a, Result<DetachedStatus, ToolDispatchError>> {
        Box::pin(async {
            Err(dispatch_error(
                DispatchStage::PreDispatch,
                DispatchProof::NotSent,
                ProviderFailureKind::InvalidRequest,
                "managed web tools never create detached operations",
            ))
        })
    }

    fn cancel<'a>(
        &'a self,
        _operation: &'a DetachedOperationId,
        _fence: Fence,
    ) -> BoxFuture<'a, Result<(), ToolDispatchError>> {
        Box::pin(async {
            Err(dispatch_error(
                DispatchStage::PreDispatch,
                DispatchProof::NotSent,
                ProviderFailureKind::InvalidRequest,
                "managed web tools have no detached operation to cancel",
            ))
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FetchArgs {
    url: String,
    #[serde(default)]
    format: Option<FetchFormat>,
    #[serde(default)]
    max_bytes: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    query: String,
    #[serde(default)]
    count: Option<u8>,
    #[serde(default)]
    country: Option<String>,
    #[serde(default)]
    freshness: Option<SearchFreshness>,
}

async fn execute_fetch(
    client: &dyn ManagedWebClient,
    input: &CanonicalJson,
) -> Result<serde_json::Value, ToolDispatchError> {
    let args: FetchArgs = serde_json::from_value(input.to_value()).map_err(input_error)?;
    let request = FetchRequest {
        url: &args.url,
        format: args.format.unwrap_or(FetchFormat::Markdown),
        max_bytes: args.max_bytes.unwrap_or(500_000),
    };
    client
        .fetch(request)
        .await
        .map_err(|error| network_error(&error))
        .and_then(result_value)
}

async fn execute_search(
    client: &dyn ManagedWebClient,
    input: &CanonicalJson,
    credential: &WebSearchCredential,
) -> Result<serde_json::Value, ToolDispatchError> {
    let args: SearchArgs = serde_json::from_value(input.to_value()).map_err(input_error)?;
    let request = WebSearchRequest {
        query: &args.query,
        count: args.count.unwrap_or(10),
        country: args.country.as_deref(),
        freshness: args.freshness,
    };
    client
        .search(request, credential)
        .await
        .map_err(|error| network_error(&error))
        .and_then(result_value)
}

fn result_value<T: serde::Serialize>(value: T) -> Result<serde_json::Value, ToolDispatchError> {
    serde_json::to_value(value).map_err(|_| {
        dispatch_error(
            DispatchStage::Terminal,
            DispatchProof::ResponseStarted,
            ProviderFailureKind::ProtocolViolation,
            "managed web result could not be encoded",
        )
    })
}

fn input_error(_error: serde_json::Error) -> ToolDispatchError {
    dispatch_error(
        DispatchStage::PreDispatch,
        DispatchProof::NotSent,
        ProviderFailureKind::InvalidRequest,
        "managed web input does not match its closed schema",
    )
}

fn credential_error(error: &CredentialSourceError) -> ToolDispatchError {
    let (kind, message) = match error {
        CredentialSourceError::Missing => (
            ProviderFailureKind::Authentication,
            "session has no admitted managed-search credential",
        ),
        CredentialSourceError::Unavailable => (
            ProviderFailureKind::Transport,
            "managed-search credential custody is unavailable",
        ),
        CredentialSourceError::Malformed => (
            ProviderFailureKind::Authentication,
            "managed-search credential is malformed",
        ),
    };
    dispatch_error(
        DispatchStage::PreDispatch,
        DispatchProof::NotSent,
        kind,
        message,
    )
}

fn network_error(error: &ManagedWebCallError) -> ToolDispatchError {
    dispatch_error(
        DispatchStage::Dispatched,
        DispatchProof::PossiblySent,
        ProviderFailureKind::Transport,
        &error.to_string(),
    )
}

fn dispatch_error(
    stage: DispatchStage,
    proof: DispatchProof,
    kind: ProviderFailureKind,
    message: &str,
) -> ToolDispatchError {
    ToolDispatchError {
        stage,
        proof,
        retryable: proof == DispatchProof::NotSent && kind != ProviderFailureKind::Cancelled,
        detail: RedactedDetail::internal(kind, message),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aex_brain_application::ports::{
        CancelToken, ControlStateView, DispatchTicket, FenceGuard, PreparedToolCall, ToolOutcome,
        ToolRoute,
    };
    use aex_brain_domain::effect::{DispatchProof, DispatchStage, EffectClass};
    use aex_brain_domain::ids::{
        AgentId, AgentKey, AgentRevision, CancelEpoch, ContentHash, EffectId, Fence, OwnerToken,
        SessionId, Timestamp, ToolCallId, ToolName,
    };
    use aex_brain_domain::journal::ExecutorRoute;
    use aex_brain_tool_catalog::router::ToolExecutor as _;
    use aex_wire::ids::{OrganizationId, PrefixedId as _, Uuid7, WorkspaceId};

    use super::{
        CredentialSourceError, ManagedWebCallError, ManagedWebClient, ManagedWebExecutor,
        WebSearchCredentialSource,
    };
    use crate::fetch::{FetchDocument, FetchRequest};
    use crate::search::{WebSearchCredential, WebSearchRequest, WebSearchResult};

    #[derive(Debug)]
    struct FixtureClient;

    impl ManagedWebClient for FixtureClient {
        fn fetch<'a>(
            &'a self,
            request: FetchRequest<'a>,
        ) -> aex_brain_application::ports::BoxFuture<'a, Result<FetchDocument, ManagedWebCallError>>
        {
            Box::pin(async move {
                Ok(FetchDocument {
                    url: request.url.to_owned(),
                    final_url: request.url.to_owned(),
                    status: 200,
                    media_type: "text/plain".to_owned(),
                    charset: Some("utf-8".to_owned()),
                    format: request.format,
                    bytes: 2,
                    truncated: false,
                    redirects: Vec::new(),
                    content: "ok".to_owned(),
                    locator: aex_wire::ids::ContentHash::of(b"ok"),
                })
            })
        }

        fn search<'a>(
            &'a self,
            request: WebSearchRequest<'a>,
            credential: &'a WebSearchCredential,
        ) -> aex_brain_application::ports::BoxFuture<'a, Result<WebSearchResult, ManagedWebCallError>>
        {
            Box::pin(async move {
                Ok(WebSearchResult {
                    provider: credential.provider(),
                    query: request.query.to_owned(),
                    results: Vec::new(),
                    truncated: false,
                })
            })
        }
    }

    #[derive(Debug)]
    struct FixtureCredentials;

    impl WebSearchCredentialSource for FixtureCredentials {
        fn resolve<'a>(
            &'a self,
            _ticket: &'a DispatchTicket,
        ) -> aex_brain_application::ports::BoxFuture<
            'a,
            Result<WebSearchCredential, CredentialSourceError>,
        > {
            Box::pin(async {
                WebSearchCredential::parse(br#"{"provider":"brave","apiKey":"test-key"}"#)
                    .map_err(|_| CredentialSourceError::Malformed)
            })
        }
    }

    fn workspace(byte: u8) -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [byte; 10]))
    }

    fn organization(byte: u8) -> OrganizationId {
        OrganizationId::from_uuid7(Uuid7::compose(1_754_051_696_789, [byte; 10]))
    }

    fn ticket() -> DispatchTicket {
        let key = AgentKey::new(
            SessionId(uuid::Uuid::from_u128(1)),
            AgentId(uuid::Uuid::from_u128(2)),
        );
        let guard = FenceGuard::new(
            key,
            OwnerToken(uuid::Uuid::from_u128(3)),
            Fence(4),
            AgentRevision(5),
            None,
            CancelEpoch::ZERO,
            CancelToken::new(),
        );
        DispatchTicket::mint(
            &guard,
            workspace(1),
            organization(2),
            EffectId([7; 16]),
            1,
            Timestamp::from_millis(1_800_000_000_000),
        )
    }

    fn call(name: &str, input: &serde_json::Value, max_result_bytes: usize) -> PreparedToolCall {
        PreparedToolCall {
            call: ToolCallId::new("call-1").expect("bounded call id"),
            route: ToolRoute {
                name: ToolName::parse(name).expect("valid tool name"),
                executor: ExecutorRoute::ManagedWeb,
                class: EffectClass::NonReplayable,
                timeout_ms: 30_000,
                concurrency_weight: 4,
                manifest_digest: ContentHash::of(b"manifest"),
            },
            input: aex_wire::CanonicalJson::from_value(input).expect("canonical input"),
            max_result_bytes,
            hands_generation: aex_wire::ids::GenerationId::from_uuid7(
                aex_wire::ids::Uuid7::compose(1, [3; 10]),
            ),
            control: ControlStateView::default(),
        }
    }

    fn executor() -> ManagedWebExecutor {
        ManagedWebExecutor::new(Arc::new(FixtureClient), Arc::new(FixtureCredentials))
    }

    #[tokio::test]
    async fn fetch_and_search_return_bounded_canonical_managed_web_receipts() {
        let executor = executor();
        for call in [
            call(
                "web_fetch",
                &serde_json::json!({"url":"https://example.com","format":"text","maxBytes":1024}),
                4_096,
            ),
            call(
                "web_search",
                &serde_json::json!({"query":"aex","count":1}),
                4_096,
            ),
        ] {
            let outcome = executor
                .invoke(&ticket(), &call, &CancelToken::new())
                .await
                .expect("the fixture completes");
            let ToolOutcome::Completed(result) = outcome else {
                panic!("managed web is never detached")
            };
            assert_eq!(result.executed_on, ExecutorRoute::ManagedWeb);
            assert!(!result.is_error);
            let serialized = aex_wire::to_jcs_bytes(&result.content).expect("canonical result");
            assert!(!serialized.is_empty());
        }
    }

    #[tokio::test]
    async fn invalid_input_and_pre_dispatch_cancellation_prove_not_sent() {
        let executor = executor();
        let invalid = call("web_fetch", &serde_json::json!({"url": 7}), 4_096);
        let error = executor
            .invoke(&ticket(), &invalid, &CancelToken::new())
            .await
            .expect_err("invalid input is refused");
        assert_eq!(error.stage, DispatchStage::PreDispatch);
        assert_eq!(error.proof, DispatchProof::NotSent);

        let cancel = CancelToken::new();
        cancel.cancel();
        let valid = call(
            "web_fetch",
            &serde_json::json!({"url":"https://example.com"}),
            4_096,
        );
        let error = executor
            .invoke(&ticket(), &valid, &cancel)
            .await
            .expect_err("cancelled calls do not dispatch");
        assert_eq!(error.stage, DispatchStage::PreDispatch);
        assert_eq!(error.proof, DispatchProof::NotSent);
        assert!(!error.retryable);
    }

    #[tokio::test]
    async fn exact_result_bound_refuses_instead_of_truncating() {
        let call = call(
            "web_fetch",
            &serde_json::json!({"url":"https://example.com"}),
            1,
        );
        let error = executor()
            .invoke(&ticket(), &call, &CancelToken::new())
            .await
            .expect_err("the result is larger than one byte");
        assert_eq!(error.stage, DispatchStage::Terminal);
        assert_eq!(error.proof, DispatchProof::ResponseStarted);
        assert!(!error.retryable);
    }
}
