//! The one rendering of a session onto the public contract.
//!
//! There is exactly one of these on purpose. A create stores the canonical 201
//! bytes on its idempotency receipt so a replay is byte-identical rather than
//! re-projected (A D-6), and that argument only holds if the bytes the winner
//! sent and the bytes a later read renders come out of the same function. A
//! second projection — one for create, one for `session_get` — would eventually
//! disagree, and the disagreement would surface as a replay that differs from
//! the original.
//!
//! So: `session_get` and `sessions_list` consume this. They must not grow their
//! own.

use aex_session_domain::{Message, MessagePart, MessageRole, MessageState, Session, SessionStatus};
use aex_wire::canonical::CanonicalError;
use aex_wire::models;

/// The public status of a session head.
///
#[must_use]
pub const fn public_status(status: SessionStatus) -> models::SessionStatus {
    match status {
        SessionStatus::Idle => models::SessionStatus::Idle,
        SessionStatus::Running => models::SessionStatus::Running,
        SessionStatus::Suspending | SessionStatus::Suspended | SessionStatus::Resuming => {
            models::SessionStatus::Idle
        }
        SessionStatus::Terminating => models::SessionStatus::Terminating,
        SessionStatus::Terminated => models::SessionStatus::Terminated,
        SessionStatus::Deleting => models::SessionStatus::Deleting,
    }
}

fn sandbox_status(session: &Session) -> models::SandboxStatus {
    use aex_session_domain::SandboxPreparationStatus;

    let enabled =
        serde_json::from_value::<models::ResolvedConfig>(session.resolved.document().to_value())
            .map_or(true, |resolved| resolved.sandbox_enabled);
    if !enabled {
        return models::SandboxStatus::Disabled;
    }
    match session.lifecycle.sandbox {
        SandboxPreparationStatus::Disabled => return models::SandboxStatus::Disabled,
        SandboxPreparationStatus::Lost => return models::SandboxStatus::Lost,
        SandboxPreparationStatus::Requested
            if matches!(
                session.lifecycle.status,
                SessionStatus::Idle | SessionStatus::Running
            ) => return models::SandboxStatus::Requested,
        SandboxPreparationStatus::Suspended
            if session.lifecycle.status == SessionStatus::Suspended =>
        {
            return models::SandboxStatus::Suspended;
        }
        SandboxPreparationStatus::Requested
        | SandboxPreparationStatus::Ready
        | SandboxPreparationStatus::Suspended => {}
    }
    match session.lifecycle.status {
        SessionStatus::Suspending => models::SandboxStatus::Suspending,
        SessionStatus::Suspended => models::SandboxStatus::Suspended,
        SessionStatus::Resuming => models::SandboxStatus::Resuming,
        SessionStatus::Terminating | SessionStatus::Terminated | SessionStatus::Deleting => {
            models::SandboxStatus::Lost
        }
        SessionStatus::Idle | SessionStatus::Running => models::SandboxStatus::Ready,
    }
}

/// Renders the exact collection projection of one canonical session head.
///
/// Provider and model come from the sealed resolved-config authority, never
/// from the eventually consistent workspace index locator.
#[must_use]
pub fn public_session_list_item(session: &Session) -> models::SessionListItem {
    models::SessionListItem {
        id: session.id,
        workspace_id: session.workspace,
        status: public_status(session.lifecycle.status),
        active_message_id: session.lifecycle.active.map(|active| active.message),
        expires_at: session.lifecycle.expires_at,
        provider: session.resolved.provider(),
        model: session.resolved.model().to_owned(),
        sandbox_status: sandbox_status(session),
        created_at: session.created_at,
        updated_at: session.updated_at,
    }
}

/// Renders one session head onto the published resource.
///
/// # Errors
///
/// Returns [`CanonicalError`] when the head's sealed resolved-configuration
/// document is not the generated `ResolvedConfig` shape. That is a corrupt
/// head, never a caller input: construction of `ResolvedConfigAuthority`
/// already refused any document that is not one.
pub fn public_session(session: &Session) -> Result<models::Session, CanonicalError> {
    let resolved: models::ResolvedConfig =
        serde_json::from_value(session.resolved.document().to_value()).map_err(|error| {
            CanonicalError::Malformed {
                reason: error.to_string(),
            }
        })?;
    Ok(models::Session {
        id: session.id,
        workspace_id: session.workspace,
        status: public_status(session.lifecycle.status),
        revision: session.revision.0,
        active_message_id: session.lifecycle.active.map(|active| active.message),
        expires_at: session.lifecycle.expires_at,
        terminated_at: session.lifecycle.terminated_at,
        sandbox_status: sandbox_status(session),
        resolved_config: resolved,
        metadata: session
            .metadata
            .as_ref()
            .map(|metadata| serde_json::from_value(metadata.document().to_value()))
            .transpose()
            .map_err(|error| CanonicalError::Malformed {
                reason: error.to_string(),
            })?,
        created_at: session.created_at,
        updated_at: session.updated_at,
    })
}

/// The exact canonical bytes a session resource is served as.
///
/// # Errors
///
/// Returns [`CanonicalError`] as [`public_session`] does, or when the rendered
/// resource cannot be canonicalized.
pub fn canonical_session_bytes(session: &Session) -> Result<Vec<u8>, CanonicalError> {
    serde_json::to_vec(&public_session(session)?).map_err(|error| CanonicalError::Malformed {
        reason: error.to_string(),
    })
}

/// Projects one complete sealed message without exposing its internal run id.
///
/// # Errors
///
/// Returns [`CanonicalError`] for an open row or the retired persisted-file
/// message part. Public admission is text-only and built-in tool records are
/// the only non-text parts in this release.
pub fn public_message(message: &Message) -> Result<models::Message, CanonicalError> {
    if message.state != MessageState::Sealed || message.sealed_at.is_none() {
        return Err(CanonicalError::Malformed {
            reason: "only complete sealed messages are public".to_owned(),
        });
    }
    let mut content = Vec::with_capacity(message.parts.len());
    for part in &message.parts {
        content.push(match part {
            MessagePart::Text { text } => {
                models::MessagePart::Text(models::MessagePartText { text: text.clone() })
            }
            MessagePart::ToolCall { id, arguments } => {
                models::MessagePart::ToolCall(models::MessagePartToolCall {
                    id: *id,
                    name: "tool".to_owned(),
                    arguments: aex_wire::CanonicalJson::from_value(
                        &serde_json::json!({"sha256": arguments.to_string()}),
                    )?,
                })
            }
            MessagePart::ToolResult { id, result } => {
                models::MessagePart::ToolResult(models::MessagePartToolResult {
                    id: *id,
                    preview: result.to_string(),
                    sandbox_path: None,
                    sha256: None,
                    size_bytes: None,
                    truncated: false,
                })
            }
            MessagePart::File { .. } => {
                return Err(CanonicalError::Malformed {
                    reason: "persisted-file message parts are retired".to_owned(),
                });
            }
        });
    }
    Ok(models::Message {
        id: message.id,
        session_id: message.session,
        role: match message.role {
            MessageRole::User => models::MessageRole::User,
            MessageRole::Assistant => models::MessageRole::Assistant,
            MessageRole::Tool => models::MessageRole::Tool,
        },
        content,
        created_at: message.created_at,
    })
}

#[cfg(test)]
mod tests {
    use aex_session_domain::{MessagePart, MessageRole, MessageState, SessionStatus};

    #[test]
    fn the_list_projection_is_complete_without_resolved_config() {
        let mut session = aex_session_domain::testing::session_fixture();
        session
            .lifecycle
            .begin_terminate(aex_session_domain::TerminationReason::User)
            .expect("termination starts");
        session
            .lifecycle
            .complete_terminate(session.updated_at)
            .expect("termination completes");
        session.lifecycle.begin_delete().expect("deletion starts");
        session.status = SessionStatus::Deleting;
        let item = super::public_session_list_item(&session);

        assert_eq!(item.id, session.id);
        assert_eq!(item.workspace_id, session.workspace);
        assert_eq!(item.status, aex_wire::models::SessionStatus::Deleting);
        assert_eq!(item.active_message_id, None);
        assert_eq!(item.expires_at, session.lifecycle.expires_at);
        assert_eq!(item.sandbox_status, aex_wire::models::SandboxStatus::Lost);
        assert_eq!(item.provider, session.resolved.provider());
        assert_eq!(item.model, session.resolved.model());
        assert_eq!(item.created_at, session.created_at);
        assert_eq!(item.updated_at, session.updated_at);
    }

    #[test]
    fn requested_setup_does_not_mask_a_lifecycle_transition() {
        let mut session = aex_session_domain::testing::session_fixture();
        let generation = session.generation.expect("sandbox generation");
        session.lifecycle = aex_session_domain::SessionLifecycle::requested(
            generation,
            session.created_at,
        )
        .expect("requested lifecycle");
        session
            .lifecycle
            .begin_terminate(aex_session_domain::TerminationReason::User)
            .expect("termination starts");
        session.status = session.lifecycle.status;

        assert_eq!(
            super::public_session_list_item(&session).sandbox_status,
            aex_wire::models::SandboxStatus::Terminating,
        );
    }

    #[test]
    fn a_sealed_message_projects_without_its_internal_run_identity() {
        let (session, run, agent, mut message) = aex_session_domain::testing::running_session();
        message.role = MessageRole::User;
        message.state = MessageState::Sealed;
        message.parts = vec![MessagePart::Text {
            text: "hello".to_owned(),
        }];
        message.sealed_at = Some(message.created_at);

        let projected = super::public_message(&message).expect("sealed message projects");
        let rendered = serde_json::to_value(&projected).expect("public message serializes");

        assert_eq!(projected.session_id, session.id);
        assert_eq!(projected.role, aex_wire::models::MessageRole::User);
        assert_eq!(projected.content.len(), 1);
        assert_eq!(message.run, Some(run.id));
        assert_eq!(message.agent, agent.id);
        assert!(rendered.get("runId").is_none());
        assert!(rendered.get("agentId").is_none());
    }

    #[test]
    fn an_open_message_is_never_public() {
        let (_, _, _, message) = aex_session_domain::testing::running_session();
        assert!(super::public_message(&message).is_err());
    }
}
