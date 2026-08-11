//! Black-box admission transaction and route-ownership requirements.

use aex_internal_contracts::RunId;
use aex_regional_http::idempotency::{IdentityContext, identity};
use aex_wire::idempotency::IdempotencyKey;
use aex_wire::ids::{MessageId, PrefixedId, SessionId, Uuid7, WorkspaceId};
use aex_wire::routes::{RouteId, TransportKind, route};
use aex_wire::types::HttpMethod;
use session_stream_api::session::admission::{
    AdmissionInput, Condition, Table, TransactionAction, compile_message_admission,
};
use session_stream_api::session::routes::session_route_ids;

fn id<T: PrefixedId>(millis: u64, seed: u8) -> T {
    T::from_uuid7(Uuid7::compose(millis, [seed; 10]))
}

fn input() -> AdmissionInput {
    AdmissionInput {
        workspace_id: id::<WorkspaceId>(1, 1),
        session_id: id::<SessionId>(2, 2),
        message_id: id::<MessageId>(3, 3),
        run_id: RunId::from_uuid7(Uuid7::compose(4, [4; 10])),
        expected_revision: 17,
        deletion_epoch: 3,
        cancellation_epoch: 9,
        max_spend_cents: 250,
    }
}

fn replay_identity() -> aex_regional_http::idempotency::IdempotencyIdentity {
    let input = input();
    identity(
        &IdentityContext {
            principal: "key_fixture",
            organization: "org_fixture",
            workspace: input.workspace_id,
            route: RouteId::SessionMessageSend,
            method: HttpMethod::Post,
        },
        &IdempotencyKey::parse("message-fixture").expect("key"),
        br#"{"parts":[{"type":"text","text":"hello"}]}"#,
    )
}

#[test]
fn admission_is_exactly_seven_actions_and_never_reserves_account_finance() {
    let plan = compile_message_admission(&replay_identity(), &input()).expect("plan");
    assert_eq!(plan.actions.len(), 7);
    assert_eq!(plan.client_request_token.as_str().len(), 36);
    assert_eq!(
        plan.actions
            .iter()
            .map(TransactionAction::table)
            .collect::<Vec<_>>(),
        vec![
            Table::AuthzProjection,
            Table::SessionAuthority,
            Table::SessionAuthority,
            Table::SessionAuthority,
            Table::RegionalWork,
            Table::SessionAuthority,
            Table::SessionAuthority,
        ]
    );
    assert_eq!(
        plan.actions,
        vec![
            TransactionAction::ConditionCheck(Condition::ProjectedEpochsAndAccountState),
            TransactionAction::PutMessage(Condition::AttributeNotExists),
            TransactionAction::PutRun(Condition::AttributeNotExists),
            TransactionAction::UpdateSessionHead(Condition::SessionHead {
                revision: 17,
                deletion_epoch: 3,
                cancellation_epoch: 9,
            }),
            TransactionAction::PutRootContinuation(Condition::AttributeNotExists),
            TransactionAction::PutIdempotencyReceipt(Condition::AttributeNotExists),
            TransactionAction::PutRunAdmittedEvent(Condition::AttributeNotExists),
        ]
    );
}

#[test]
fn the_client_request_token_is_deterministic_and_intent_bound() {
    let first = compile_message_admission(&replay_identity(), &input()).expect("plan");
    let second = compile_message_admission(&replay_identity(), &input()).expect("plan");
    assert_eq!(first.client_request_token, second.client_request_token);
    let changed = identity(
        &IdentityContext {
            principal: "key_fixture",
            organization: "org_fixture",
            workspace: input().workspace_id,
            route: RouteId::SessionMessageSend,
            method: HttpMethod::Post,
        },
        &IdempotencyKey::parse("message-fixture").expect("key"),
        br#"{"parts":[]}"#,
    );
    assert_ne!(
        first.client_request_token,
        compile_message_admission(&changed, &input())
            .expect("plan")
            .client_request_token
    );
}

#[test]
fn session_route_partition_contains_no_secret_plaintext_or_stream_mutation() {
    let routes = session_route_ids();
    assert!(routes.contains(&RouteId::SecretGet));
    assert!(routes.contains(&RouteId::SecretsList));
    assert!(routes.contains(&RouteId::ProviderCredentialGet));
    assert!(!routes.contains(&RouteId::SecretPut));
    assert!(!routes.contains(&RouteId::ProviderCredentialRegister));
    assert!(!routes.contains(&RouteId::SessionObservationsEventsStream));
    assert!(
        routes
            .iter()
            .all(|id| route(*id).transport == TransportKind::Unary)
    );
}
