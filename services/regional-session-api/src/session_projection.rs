//! Total projection from the canonical session authority to strict-v1 wire models.

use std::collections::BTreeMap;

use aex_session_domain::{
    CloneFiles, MessagePart, MessageState, RunOutcome, SessionMetadata, SessionStatus,
};
use aex_wire::models;
use aex_wire::types::MetadataValue;

/// A stored authority value cannot enter its generated public model.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProjectionError {
    /// The stored resolved document no longer decodes as strict-v1.
    #[error("stored resolved configuration is not strict-v1")]
    ResolvedConfig,
    /// Validated metadata no longer projects as the closed scalar vocabulary.
    #[error("stored session metadata is outside the closed scalar vocabulary")]
    Metadata,
}

/// Projects one complete session.
///
/// # Errors
///
/// Returns [`ProjectionError`] for a corrupt canonical document; it never
/// substitutes defaults or drops a field.
pub fn session(value: &aex_session_domain::Session) -> Result<models::Session, ProjectionError> {
    let resolved_config = serde_json::from_value(value.resolved.document().to_value())
        .map_err(|_| ProjectionError::ResolvedConfig)?;
    Ok(models::Session {
        continuity: models::WorkspaceContinuity {
            last_persisted_at: value.last_persisted_at,
            live_generation_id: value.generation,
            persist_revision: value.persist_revision.0,
            root_hash: Some(aex_session_domain::public_root_hash(&value.persisted_root)),
        },
        created_at: value.created_at,
        id: value.id,
        lineage: lineage(value.lineage),
        metadata: value.metadata.as_ref().map(metadata).transpose()?,
        resolved_config,
        revision: value.revision.0,
        status: session_status(value.status),
        updated_at: value.updated_at,
        workspace_id: value.workspace,
    })
}

/// Projects the bounded workspace-list shape without loading another row.
#[must_use]
pub fn session_list_item(value: &aex_session_domain::Session) -> models::SessionListItem {
    models::SessionListItem {
        created_at: value.created_at,
        id: value.id,
        model: value.resolved.model().to_owned(),
        provider: value.resolved.provider(),
        revision: value.revision.0,
        status: session_status(value.status),
        updated_at: value.updated_at,
        workspace_id: value.workspace,
    }
}

/// Projects a sealed message. Open agent output is deliberately absent until
/// the terminal barrier seals it, so a torn assistant message is never public.
#[must_use]
pub fn message(value: &aex_session_domain::Message) -> Option<models::Message> {
    if value.state != MessageState::Sealed {
        return None;
    }
    Some(models::Message {
        content: value.parts.iter().map(message_part).collect(),
        created_at: value.created_at,
        id: value.id,
        role: match value.role {
            aex_session_domain::MessageRole::User => models::MessageRole::User,
            aex_session_domain::MessageRole::Assistant => models::MessageRole::Assistant,
            aex_session_domain::MessageRole::Tool => models::MessageRole::Tool,
        },
        run_id: value.run,
        session_id: value.session,
    })
}

/// Projects a run without inventing request identity or telemetry completeness.
#[must_use]
pub fn run(value: &aex_session_domain::Run) -> models::Run {
    let (output_message_ids, error) = match value.outcome.as_ref() {
        Some(RunOutcome::Succeeded { output_messages }) => (Some(output_messages.clone()), None),
        Some(RunOutcome::Failed { error }) => (
            None,
            Some(models::RunFailure {
                code: error.code.into(),
                detail: error.detail.clone(),
                message: error.message.clone(),
                retryable: error.retryable,
            }),
        ),
        Some(
            RunOutcome::TimedOut { .. } | RunOutcome::Cancelled { .. } | RunOutcome::Interrupted(_),
        )
        | None => (None, None),
    };
    models::Run {
        error,
        id: value.id,
        max_spend_cents: aex_wire::types::Cents::new(value.max_spend_cents.get()),
        message_id: value.message,
        output_message_ids,
        queued_at: value.queued_at,
        session_id: value.session,
        started_at: value.started_at,
        status: match value.status {
            aex_session_domain::RunStatus::Queued => models::RunStatus::Queued,
            aex_session_domain::RunStatus::Running => models::RunStatus::Running,
            aex_session_domain::RunStatus::Succeeded => models::RunStatus::Succeeded,
            aex_session_domain::RunStatus::Failed => models::RunStatus::Failed,
            aex_session_domain::RunStatus::TimedOut => models::RunStatus::TimedOut,
            aex_session_domain::RunStatus::Cancelled => models::RunStatus::Cancelled,
            aex_session_domain::RunStatus::Interrupted => models::RunStatus::Interrupted,
        },
        telemetry_complete: value.telemetry_complete,
        telemetry_gap_ids: value.telemetry_gaps.clone(),
        terminal_at: value.terminal_at,
    }
}

fn message_part(part: &MessagePart) -> models::MessagePart {
    match part {
        MessagePart::Text { text } => {
            models::MessagePart::Text(models::MessagePartText { text: text.clone() })
        }
        MessagePart::File { path, media_type } => {
            models::MessagePart::File(models::MessagePartFile {
                media_type: media_type.clone(),
                path: path.clone(),
                source: models::FileSource::Persisted,
            })
        }
        MessagePart::ToolCall { id, arguments } => {
            models::MessagePart::ToolCall(models::MessagePartToolCall {
                arguments_digest: *arguments,
                id: *id,
            })
        }
        MessagePart::ToolResult { id, result } => {
            models::MessagePart::ToolResult(models::MessagePartToolResult {
                id: *id,
                result_digest: *result,
            })
        }
    }
}

/// Total admission mapping from the generated message-part vocabulary.
#[must_use]
pub fn message_part_from_wire(part: models::MessagePart) -> MessagePart {
    match part {
        models::MessagePart::Text(part) => MessagePart::Text { text: part.text },
        models::MessagePart::File(part) => {
            let models::FileSource::Persisted = part.source;
            MessagePart::File {
                path: part.path,
                media_type: part.media_type,
            }
        }
        models::MessagePart::ToolCall(part) => MessagePart::ToolCall {
            id: part.id,
            arguments: part.arguments_digest,
        },
        models::MessagePart::ToolResult(part) => MessagePart::ToolResult {
            id: part.id,
            result: part.result_digest,
        },
    }
}

const fn session_status(status: SessionStatus) -> models::SessionStatus {
    match status {
        SessionStatus::Idle => models::SessionStatus::Idle,
        SessionStatus::Running => models::SessionStatus::Running,
        SessionStatus::AwaitingApproval => models::SessionStatus::AwaitingApproval,
        SessionStatus::Trashed | SessionStatus::Purging => models::SessionStatus::Deleting,
    }
}

fn lineage(value: aex_session_domain::Lineage) -> models::SessionLineage {
    match value.origin {
        Some(origin) => models::SessionLineage {
            clone_operation_id: Some(origin.operation),
            cloned_at_persist_revision: Some(origin.source_persist_revision.0),
            origin_session_id: Some(origin.session),
        },
        None => models::SessionLineage {
            clone_operation_id: None,
            cloned_at_persist_revision: None,
            origin_session_id: None,
        },
    }
}

fn metadata(value: &SessionMetadata) -> Result<BTreeMap<String, MetadataValue>, ProjectionError> {
    let document = value.document().to_value();
    let object = document.as_object().ok_or(ProjectionError::Metadata)?;
    object
        .iter()
        .map(|(key, value)| {
            let projected = if let Some(text) = value.as_str() {
                MetadataValue::Text(text.to_owned())
            } else if let Some(value) = value.as_bool() {
                MetadataValue::Bool(value)
            } else if value.is_null() {
                MetadataValue::Null
            } else if let Some(value) = value.as_f64() {
                MetadataValue::Number(value)
            } else {
                return Err(ProjectionError::Metadata);
            };
            Ok((key.clone(), projected))
        })
        .collect()
}

/// Total mapping from the generated clone file vocabulary.
#[must_use]
pub const fn clone_files(value: models::CloneFiles) -> CloneFiles {
    match value {
        models::CloneFiles::Current => CloneFiles::Current,
        models::CloneFiles::Initial => CloneFiles::Initial,
        models::CloneFiles::None => CloneFiles::None,
    }
}

#[cfg(test)]
mod tests {
    use aex_content_domain::ContentDigest;
    use aex_session_domain::testing::{id, running_session, session_fixture};
    use aex_session_domain::{MessagePart, MessageState};
    use aex_wire::ids::{FilePath, ToolCallId};

    use super::{message, message_part_from_wire, run, session, session_list_item};

    #[test]
    fn full_and_list_session_projections_share_authoritative_provider_model() {
        let stored = session_fixture();
        let full = session(&stored).expect("projects");
        let list = session_list_item(&stored);
        assert_eq!(full.resolved_config.provider, list.provider);
        assert_eq!(full.resolved_config.model, list.model);
        assert_eq!(full.revision, list.revision);
    }

    #[test]
    fn every_durable_message_part_projects_without_loss() {
        let (_session, _run, _agent, mut stored) = running_session();
        stored.state = MessageState::Sealed;
        stored.parts = vec![
            MessagePart::Text {
                text: "hello".to_owned(),
            },
            MessagePart::File {
                path: FilePath::parse("/input.txt").expect("path"),
                media_type: Some("text/plain".to_owned()),
            },
            MessagePart::ToolCall {
                id: id::<ToolCallId>(41),
                arguments: ContentDigest::of(b"args"),
            },
            MessagePart::ToolResult {
                id: id::<ToolCallId>(41),
                result: ContentDigest::of(b"result"),
            },
        ];
        let projected = message(&stored).expect("sealed");
        assert_eq!(projected.content.len(), 4);
        let restored: Vec<_> = projected
            .content
            .into_iter()
            .map(message_part_from_wire)
            .collect();
        assert_eq!(restored, stored.parts);
    }

    #[test]
    fn open_messages_are_not_published_and_unsettled_telemetry_is_not_invented() {
        let (_session, stored_run, _agent, open) = running_session();
        assert!(message(&open).is_none());
        let projected = run(&stored_run);
        assert_eq!(projected.telemetry_complete, None);
        assert_eq!(projected.telemetry_gap_ids, None);
    }
}
