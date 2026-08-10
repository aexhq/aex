use std::sync::{Arc, Mutex};

use aex_brain_app::ports::{
    CancelToken, ControlStateView, DispatchTicket, FenceGuard, PreparedToolCall, ToolOutcome,
    ToolRoute,
};
use aex_brain_domain::effect::{DispatchProof, DispatchStage, EffectClass};
use aex_brain_domain::ids::{
    AgentId, AgentKey, AgentRevision, CancelEpoch, ContentHash, EffectId, Fence, OwnerToken,
    SessionId, Timestamp, ToolCallId, ToolName,
};
use aex_brain_domain::journal::ExecutorRoute;
use aex_brain_tool_catalog::catalog::BUILTIN_CATALOG_DIGEST;
use aex_brain_tool_catalog::router::ToolExecutor as _;
use aex_identity_domain::assertion::{Assertion, KeyId, LocalSigner, Plane};
use aex_internal_contracts::SchemaVersion;
use aex_internal_contracts::tool_exec::{
    ToolExecRequest, ToolExecResponse, ToolResultPart as WireToolResultPart,
};
use aex_wire::ids::{
    ContentHash as WireContentHash, OrganizationId, PrefixedId as _, Uuid7, WorkspaceId,
};
use aex_wire::types::Region;
use base64::Engine as _;
use zeroize::Zeroizing;

use super::{
    BindingError, ToolExecExecutor, ToolExecTransportError, Transport, parse_signing_secret,
    validate_endpoint,
};

#[derive(Debug)]
struct FixtureTransport {
    response: ToolExecResponse,
    seen: Mutex<Vec<ToolExecRequest>>,
}

impl Transport for FixtureTransport {
    fn execute<'a>(
        &'a self,
        request: &'a ToolExecRequest,
        _max_result_bytes: usize,
    ) -> aex_brain_app::ports::BoxFuture<'a, Result<ToolExecResponse, ToolExecTransportError>> {
        Box::pin(async move {
            self.seen
                .lock()
                .expect("seen requests")
                .push(request.clone());
            Ok(self.response.clone())
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

fn call() -> PreparedToolCall {
    PreparedToolCall {
        call: ToolCallId::new("call-1").expect("bounded call id"),
        route: ToolRoute {
            name: ToolName::parse("web_search").expect("tool name"),
            executor: ExecutorRoute::ToolExec,
            class: EffectClass::NonReplayable,
            timeout_ms: 15_000,
            concurrency_weight: 4,
            manifest_digest: ContentHash::of(
                &aex_brain_tool_catalog::catalog::builtin_catalog_bytes()
                    .expect("built-in catalog"),
            ),
        },
        input: aex_wire::CanonicalJson::parse(r#"{"query":"aex"}"#).expect("canonical arguments"),
        max_result_bytes: 4_096,
        hands_generation: aex_wire::ids::GenerationId::from_uuid7(Uuid7::compose(1, [3; 10])),
        control: ControlStateView::default(),
    }
}

fn executor(response: ToolExecResponse) -> (ToolExecExecutor, Arc<FixtureTransport>) {
    let transport = Arc::new(FixtureTransport {
        response,
        seen: Mutex::new(Vec::new()),
    });
    let signer = LocalSigner::new(
        KeyId::new(uuid::Uuid::from_u128(9)),
        &Zeroizing::new([7_u8; 32]),
    );
    let certified_manifest =
        WireContentHash::parse(BUILTIN_CATALOG_DIGEST).expect("certified manifest");
    let route_manifest = ContentHash::of(
        &aex_brain_tool_catalog::catalog::builtin_catalog_bytes().expect("built-in catalog"),
    );
    (
        ToolExecExecutor::new(
            transport.clone(),
            Arc::new(signer),
            Plane::Dev,
            Region::EuWest1,
            certified_manifest,
            route_manifest,
        ),
        transport,
    )
}

fn completed(text: &str, corrupt_checksum: bool) -> ToolExecResponse {
    let content = vec![WireToolResultPart::Text {
        text: text.to_owned(),
    }];
    let bytes = serde_json::to_vec(&content).expect("wire result");
    let mut checksum = ContentHash::of(&bytes).0;
    if corrupt_checksum {
        checksum[0] ^= 1;
    }
    ToolExecResponse::Completed {
        schema_version: SchemaVersion::V1,
        content,
        is_error: false,
        duration_ms: 12,
        checksum: WireContentHash::from_bytes(checksum),
    }
}

#[test]
fn only_the_private_one_route_endpoint_is_accepted() {
    assert_eq!(
        validate_endpoint("http://tool-executor.aex-dev.internal:8080/internal/tool-exec"),
        Ok(())
    );
    for endpoint in [
        "https://tool-executor.aex-dev.internal/internal/tool-exec",
        "http://tool-executor.aex-dev.internal:8080/",
        "http://example.com/internal/tool-exec",
        "http://user@tool-executor.aex-dev.internal/internal/tool-exec",
    ] {
        assert_eq!(
            validate_endpoint(endpoint),
            Err(BindingError::Endpoint),
            "{endpoint}"
        );
    }
}

#[test]
fn the_signing_secret_reuses_the_canonical_assertion_key_shape() {
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([7_u8; 32]);
    assert_eq!(
        *parse_signing_secret(&encoded).expect("canonical key"),
        [7_u8; 32]
    );
    assert_eq!(
        parse_signing_secret("not a key"),
        Err(BindingError::SecretMalformed)
    );
    assert_eq!(
        parse_signing_secret(&format!("{encoded}\n")),
        Err(BindingError::SecretMalformed),
        "the stored secret is the canonical key, not a whitespace-tolerant document"
    );
}

#[tokio::test]
async fn the_signed_executor_result_becomes_the_actual_llm_tool_result() {
    let (executor, transport) = executor(completed("actual execution result", false));
    let outcome = executor
        .invoke(&ticket(), &call(), &CancelToken::new())
        .await
        .expect("executor completes");
    let ToolOutcome::Completed(result) = outcome else {
        panic!("platform search is synchronous")
    };
    let aex_brain_domain::wire_pending::ToolResultPart::Text { text } = &result.content[0] else {
        panic!("executor returned the typed text result")
    };
    assert_eq!(text.as_str(), "actual execution result");
    assert_eq!(result.executed_on, ExecutorRoute::ToolExec);

    let seen = transport.seen.lock().expect("seen requests");
    let request = seen.first().expect("one request");
    assert_eq!(request.tool.as_str(), "web_search");
    assert_eq!(request.arguments_jcs.as_bytes(), br#"{"query":"aex"}"#);
    assert_eq!(request.effect.get(), &[7; 16]);
    assert_eq!(request.attempt, 1);
    assert_eq!(request.manifest.to_string(), BUILTIN_CATALOG_DIGEST);
    assert!(
        Assertion::from_base64url(request.assertion.as_str()).is_ok(),
        "the client must send the existing signed envelope, not an unsigned tenant field"
    );
}

#[tokio::test]
async fn a_route_from_another_catalog_cannot_borrow_the_certified_manifest() {
    let (executor, transport) = executor(completed("must not run", false));
    let mut call = call();
    call.route.manifest_digest = ContentHash::of(b"another catalog");

    let error = executor
        .invoke(&ticket(), &call, &CancelToken::new())
        .await
        .expect_err("the route and certified catalog disagree");
    assert_eq!(error.stage, DispatchStage::PreDispatch);
    assert_eq!(error.proof, DispatchProof::NotSent);
    assert!(transport.seen.lock().expect("seen requests").is_empty());
}

#[tokio::test]
async fn a_result_whose_checksum_does_not_cover_its_body_is_never_journalled() {
    let (executor, _) = executor(completed("tampered", true));
    let error = executor
        .invoke(&ticket(), &call(), &CancelToken::new())
        .await
        .expect_err("the body and checksum disagree");
    assert_eq!(error.stage, DispatchStage::Streaming);
    assert_eq!(error.proof, DispatchProof::ResponseStarted);
}
