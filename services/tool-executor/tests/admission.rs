//! What the executor accepts, and everything it refuses before spending.
//!
//! The order under test is the one the placement record fixed: every check that
//! can refuse runs before the one that costs money.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aex_control_domain::epoch::{Epoch, EpochSubjectKind};
use aex_control_domain::scope::ScopeSet;
use aex_identity_domain::assertion::{
    ASSERTION_MAX_LIFETIME_MS, AssertedAccountState, AssertionClaims, AssertionSigner, Audience,
    EpochSlot, EpochSlots, KeyId, LocalSigner, Plane, PrincipalKind, VerificationKey,
    VerificationKeySet, agent_session_binding, issue,
};
use aex_internal_contracts::SchemaVersion;
use aex_internal_contracts::assertion::AssertionAudience;
use aex_internal_contracts::tool_exec::{
    ArgumentsJcs, DeadlineMs, EffectRef, ToolExecRefusal, ToolExecRequest, ToolExecResponse,
    ToolResultPart,
};
use aex_wire::ids::{ContentHash, PrefixedId as _, ResourceName, Uuid7};
use aex_wire::types::Region;
use async_trait::async_trait;
use tool_executor::admit::Admitter;
use tool_executor::handler::Executor;
use tool_executor::run::{RunRefusal, ToolRun, ToolRunner};
use tool_executor::spend::{CeilingRefusal, OrganizationCeiling, SpendPermit};
use uuid::Uuid;
use zeroize::Zeroizing;

const NOW_MS: u64 = 1_767_225_600_000;
const ORGANIZATION: u128 = 0x0192_3f2a_1c00_7000_8000_0000_0000_0002;
const WORKSPACE: u128 = 0x0192_3f2a_1c00_7000_8000_0000_0000_0003;
const SESSION: u128 = 0x0192_3f2a_1c00_7000_8000_0000_0000_0001;
const EFFECT: [u8; 16] = [0xab; 16];

fn at() -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(NOW_MS + 1)
}

fn signer() -> LocalSigner {
    LocalSigner::new(
        KeyId::new(Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_00aa)),
        &Zeroizing::new([7_u8; 32]),
    )
}

/// The key set the executor holds. `not_after_ms` is far enough out that no test
/// accidentally proves expiry when it meant to prove something else.
fn keys(signer: &LocalSigner) -> VerificationKeySet {
    VerificationKeySet::new(vec![VerificationKey {
        kid: signer.kid(),
        public_key: signer.public_key(),
        not_after_ms: NOW_MS + 86_400_000,
    }])
    .expect("one key")
}

fn claims() -> AssertionClaims {
    AssertionClaims {
        issued_at_ms: NOW_MS,
        expires_at_ms: NOW_MS + ASSERTION_MAX_LIFETIME_MS,
        audience: Audience {
            plane: Plane::Prd,
            region: Region::EuWest1,
            service: AssertionAudience::ToolExec,
        },
        principal_kind: PrincipalKind::AgentSession,
        principal_id: Uuid::from_u128(SESSION),
        credential_binding: agent_session_binding(Uuid::from_bytes(EFFECT), 0),
        organization_id: Uuid::from_u128(ORGANIZATION),
        workspace_id: Uuid::from_u128(WORKSPACE),
        workspace_region: Region::EuWest1,
        account_state: AssertedAccountState::Active,
        scopes: ScopeSet::EMPTY,
        epochs: EpochSlots::EMPTY,
    }
}

fn request_from(signer: &LocalSigner, claims: &AssertionClaims, attempt: u16) -> ToolExecRequest {
    let assertion = issue(signer, claims).expect("the claim set signs");
    ToolExecRequest {
        schema_version: SchemaVersion::V1,
        assertion: assertion.to_issued().expect("encodes"),
        tool: ResourceName::parse("web_search").expect("a tool name"),
        manifest: ContentHash::from_bytes([9; 32]),
        arguments_jcs: ArgumentsJcs::new(br#"{"query":"aex"}"#.to_vec()).expect("inside the bound"),
        effect: EffectRef::new(EFFECT),
        attempt,
        deadline_ms: DeadlineMs::new(15_000).expect("inside the bound"),
    }
}

/// A ceiling that records whether it was consulted, so a test can prove the
/// order rather than only the outcome.
struct RecordingCeiling {
    calls: AtomicUsize,
    outcome: Option<CeilingRefusal>,
}

impl RecordingCeiling {
    fn admitting() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            outcome: None,
        })
    }

    fn refusing(refusal: CeilingRefusal) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            outcome: Some(refusal),
        })
    }
}

#[async_trait]
impl OrganizationCeiling for RecordingCeiling {
    async fn admit(
        &self,
        organization: &aex_wire::ids::OrganizationId,
        at: SystemTime,
    ) -> Result<SpendPermit, CeilingRefusal> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match &self.outcome {
            Some(refusal) => Err(refusal.clone()),
            None => {
                // A production ceiling mints the permit; a fake cannot, because
                // the fields are private to the module that checks. So the fake
                // delegates to a real in-memory ceiling instead of forging one,
                // which is exactly the property under test.
                tool_executor::spend::InMemoryOrganizationCeiling::unbounded()
                    .admit(organization, at)
                    .await
            }
        }
    }
}

/// A runner that records whether the credential path was reached.
struct RecordingRunner {
    calls: AtomicUsize,
}

impl RecordingRunner {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl ToolRunner for RecordingRunner {
    fn supports(&self, tool: &ResourceName) -> bool {
        tool.as_str() == "web_search"
    }

    async fn run(
        &self,
        _permit: &SpendPermit,
        tool: &ResourceName,
        _arguments: &ArgumentsJcs,
    ) -> Result<ToolRun, RunRefusal> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if !self.supports(tool) {
            return Err(RunRefusal::Unsupported {
                tool: tool.as_str().to_owned(),
            });
        }
        Ok(ToolRun {
            content: vec![ToolResultPart::Text {
                text: "{}".to_owned(),
            }],
            is_error: false,
        })
    }
}

fn executor(
    signer: &LocalSigner,
    ceiling: Arc<RecordingCeiling>,
    runner: Arc<RecordingRunner>,
) -> Executor {
    Executor::new(
        Admitter::new(keys(signer), Plane::Prd, Region::EuWest1),
        ceiling,
        runner,
    )
}

#[tokio::test]
async fn a_well_formed_call_resolves_its_tenant_from_the_envelope_and_runs() {
    let signer = signer();
    let ceiling = RecordingCeiling::admitting();
    let runner = RecordingRunner::new();
    let executor = executor(&signer, Arc::clone(&ceiling), Arc::clone(&runner));

    let response = executor
        .execute(&request_from(&signer, &claims(), 0), at())
        .await;
    assert!(
        matches!(
            response,
            ToolExecResponse::Completed {
                is_error: false,
                ..
            }
        ),
        "{response:?}"
    );
    assert_eq!(ceiling.calls.load(Ordering::SeqCst), 1);
    assert_eq!(runner.calls.load(Ordering::SeqCst), 1);

    // The tenant came from the envelope. Nothing in the request said it, and the
    // admitter is the only thing that could have.
    let admitter = Admitter::new(keys(&signer), Plane::Prd, Region::EuWest1);
    let call = admitter
        .admit(&request_from(&signer, &claims(), 0), NOW_MS + 1)
        .expect("admits");
    assert_eq!(
        call.organization().encode().as_str(),
        aex_wire::ids::OrganizationId::from_uuid7(
            Uuid7::from_bytes(Uuid::from_u128(ORGANIZATION).into_bytes()).expect("a v7 id")
        )
        .encode()
        .as_str()
    );
    assert_eq!(call.session(), Uuid::from_u128(SESSION));
}

#[tokio::test]
async fn every_refusal_happens_before_the_credential_is_touched() {
    let signer = signer();

    // Each row is one broken thing, and the assertion is the same every time:
    // the runner was never reached.
    let mut rows: Vec<(&str, ToolExecRequest)> = Vec::new();

    let mut expired = claims();
    expired.issued_at_ms = NOW_MS - 60_000;
    expired.expires_at_ms = NOW_MS - 30_000;
    rows.push(("expired", request_from(&signer, &expired, 0)));

    let mut elsewhere = claims();
    elsewhere.audience.service = AssertionAudience::RegionalSession;
    elsewhere.principal_kind = PrincipalKind::WorkspaceKey;
    // A workspace-key envelope for a regional edge is issuable; it simply is not
    // for this audience, and this is the case that matters most, because it is
    // the one `central-authz` could actually mint.
    rows.push(("addressed elsewhere", request_from(&signer, &elsewhere, 0)));

    let mut other_plane = claims();
    other_plane.audience.plane = Plane::Dev;
    rows.push(("another plane", request_from(&signer, &other_plane, 0)));

    let mut paused = claims();
    paused.account_state = AssertedAccountState::PausedTopUpRequired;
    rows.push(("a paused account", request_from(&signer, &paused, 0)));

    let mut with_epoch = claims();
    with_epoch.epochs = EpochSlots::new(&[EpochSlot {
        kind: EpochSubjectKind::Workspace,
        id: Uuid::from_u128(WORKSPACE),
        epoch: Epoch::new(1),
    }])
    .expect("one subject");
    rows.push((
        "an epoch this process cannot check",
        request_from(&signer, &with_epoch, 0),
    ));

    // The envelope was minted for attempt 0; presenting it for attempt 1 is a
    // captured envelope being re-aimed at a different call.
    rows.push(("a re-aimed envelope", request_from(&signer, &claims(), 1)));

    let stranger = LocalSigner::new(
        KeyId::new(Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_00bb)),
        &Zeroizing::new([9_u8; 32]),
    );
    rows.push(("an unknown key", request_from(&stranger, &claims(), 0)));

    for (what, request) in rows {
        let ceiling = RecordingCeiling::admitting();
        let runner = RecordingRunner::new();
        let executor = executor(&signer, Arc::clone(&ceiling), Arc::clone(&runner));
        let response = executor.execute(&request, at()).await;
        assert_eq!(
            response,
            ToolExecResponse::Refused {
                schema_version: SchemaVersion::V1,
                reason: ToolExecRefusal::NotAuthorized,
            },
            "{what} was not refused"
        );
        assert_eq!(
            ceiling.calls.load(Ordering::SeqCst),
            0,
            "{what} reached the ceiling store"
        );
        assert_eq!(
            runner.calls.load(Ordering::SeqCst),
            0,
            "{what} reached the vendor credential"
        );
    }
}

#[tokio::test]
async fn a_full_ceiling_refuses_before_the_credential_and_says_nothing_about_the_budget() {
    let signer = signer();
    let ceiling = RecordingCeiling::refusing(CeilingRefusal::LimitExceeded);
    let runner = RecordingRunner::new();
    let executor = executor(&signer, Arc::clone(&ceiling), Arc::clone(&runner));

    let response = executor
        .execute(&request_from(&signer, &claims(), 0), at())
        .await;
    assert_eq!(
        response,
        ToolExecResponse::Refused {
            schema_version: SchemaVersion::V1,
            reason: ToolExecRefusal::LimitExceeded,
        }
    );
    assert_eq!(ceiling.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        0,
        "the vendor credential was touched after the ceiling refused"
    );

    let document = serde_json::to_string(&response).expect("encodes");
    assert!(!document.contains("remaining"), "{document}");
    assert!(!document.contains("600"), "{document}");
}

#[tokio::test]
async fn an_unreadable_ceiling_refuses_rather_than_admits() {
    // A ceiling nobody could read is how an organization keeps spending. The
    // answer is the same refusal a full ceiling gives, because a caller cannot
    // act differently on the difference and a hostile one should not learn it.
    let signer = signer();
    let ceiling = RecordingCeiling::refusing(CeilingRefusal::Unavailable("no table".to_owned()));
    let runner = RecordingRunner::new();
    let executor = executor(&signer, Arc::clone(&ceiling), Arc::clone(&runner));

    let response = executor
        .execute(&request_from(&signer, &claims(), 0), at())
        .await;
    assert_eq!(
        response,
        ToolExecResponse::Refused {
            schema_version: SchemaVersion::V1,
            reason: ToolExecRefusal::LimitExceeded,
        }
    );
    assert_eq!(runner.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_process_with_no_verification_key_is_not_ready() {
    let executor = Executor::new(
        Admitter::new(
            VerificationKeySet::new(Vec::new()).expect("an empty set is representable"),
            Plane::Prd,
            Region::EuWest1,
        ),
        RecordingCeiling::admitting(),
        RecordingRunner::new(),
    );
    assert!(
        !executor.is_ready(),
        "a process that would refuse every request must not report ready"
    );
}
