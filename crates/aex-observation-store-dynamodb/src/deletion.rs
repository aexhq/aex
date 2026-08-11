//! Session-scoped observation deletion authority.
//!
//! The public session operation owns *when* irreversible deletion is allowed.
//! This module owns the regional observation participant: raise the exact
//! session fence, enqueue the bounded physical deletion, and report only the
//! durable state proved by the observation authority. The request is one
//! conditional write. There is no moment between "fenced" and "scheduled" in
//! which an admission can slip through.

use std::collections::HashMap;

use aex_observation_domain::frontier::DeletionState;
use aex_observation_domain::keys::{self, ControlDomain, ScopeKey};
use aex_session_dynamodb::attr::{PK, SK};
use aex_session_dynamodb::error::{Idempotence, Resolution, StoreError, classify};
use aex_wire::ids::{OperationId, SessionId, WorkspaceId};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::types::AttributeValue;

/// Durable status returned to the session deletion orchestrator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionObservationDeletionStatus {
    /// The fence is up and physical rows/objects are being drained.
    Deleting,
    /// A separate duty is proving every owned partition empty.
    Verifying,
    /// The observation authority sealed the exact-session tombstone.
    Complete,
}

impl SessionObservationDeletionStatus {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "deleting" => Some(Self::Deleting),
            "verifying" => Some(Self::Verifying),
            "complete" => Some(Self::Complete),
            _ => None,
        }
    }
}

/// How a deletion request resolved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionObservationDeletionOutcome {
    /// This request raised the fence and enqueued physical deletion.
    Started,
    /// The exact request was already durable.
    Replay(SessionObservationDeletionStatus),
}

/// One irreversible exact-session deletion request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionObservationDeletionRequest {
    /// Owning workspace.
    pub workspace: WorkspaceId,
    /// Session whose telemetry payload is deleted.
    pub session: SessionId,
    /// Canonical `session_delete` operation coordinating the participant.
    pub operation: OperationId,
    /// Shared orchestration instant.
    pub now: Timestamp,
}

/// Typed refusal from the observation deletion participant.
#[derive(Debug, thiserror::Error)]
pub enum SessionObservationDeletionError {
    /// The same session fence already belongs to another operation.
    #[error("the session observation deletion belongs to another operation")]
    Conflict,
    /// A durable row was missing or did not have the authority shape.
    #[error("the session observation deletion authority is corrupt: {detail}")]
    Corrupt {
        /// Exact invariant that was absent or contradictory.
        detail: String,
    },
    /// The requested deletion has not been admitted.
    #[error("the session observation deletion does not exist")]
    NotFound,
    /// Provider/store failure.
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Port consumed by a `session_delete` continuation.
#[async_trait::async_trait]
pub trait SessionObservationDeletion: Send + Sync {
    /// Raises the fence and schedules the exact-session deletion, idempotently.
    async fn request(
        &self,
        request: SessionObservationDeletionRequest,
    ) -> Result<SessionObservationDeletionOutcome, SessionObservationDeletionError>;

    /// Reads the durable participant status for the coordinating operation.
    async fn status(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        operation: OperationId,
    ) -> Result<SessionObservationDeletionStatus, SessionObservationDeletionError>;
}

/// DynamoDB implementation of the exact-session deletion participant.
#[derive(Clone, Debug)]
pub struct SessionObservationDeletionStore {
    dynamodb: aws_sdk_dynamodb::Client,
    table: String,
    duty_shards: u8,
}

impl SessionObservationDeletionStore {
    /// Binds the observation table and the deployed deletion-duty shard count.
    ///
    /// # Errors
    ///
    /// A zero shard count cannot address the due index.
    pub fn new(
        dynamodb: aws_sdk_dynamodb::Client,
        table: impl Into<String>,
        duty_shards: u8,
    ) -> Result<Self, SessionObservationDeletionError> {
        if duty_shards == 0 {
            return Err(SessionObservationDeletionError::Corrupt {
                detail: "the deletion duty shard count is zero".to_owned(),
            });
        }
        Ok(Self {
            dynamodb,
            table: table.into(),
            duty_shards,
        })
    }

    fn scope(workspace: WorkspaceId, session: SessionId) -> ScopeKey {
        ScopeKey::Session { workspace, session }
    }

    async fn load(
        &self,
        scope: ScopeKey,
    ) -> Result<Option<HashMap<String, AttributeValue>>, SessionObservationDeletionError> {
        self.dynamodb
            .get_item()
            .table_name(&self.table)
            .key(PK, AttributeValue::S(keys::frontier_pk(&scope)))
            .key(SK, AttributeValue::S(keys::DELETION_SK.to_owned()))
            .consistent_read(true)
            .send()
            .await
            .map(|output| output.item)
            .map_err(|error| classify(&error, Idempotence::Read).into())
    }

    fn resolve(
        item: &HashMap<String, AttributeValue>,
        scope: ScopeKey,
        operation: OperationId,
    ) -> Result<SessionObservationDeletionStatus, SessionObservationDeletionError> {
        if string(item, "itemType") != Some("scope_deletion")
            || string(item, "scopeKey") != Some(scope.to_key().as_str())
            || string(item, "workspaceId") != Some(scope.workspace().to_string().as_str())
            || scope.session().is_none_or(|session| {
                string(item, "sessionId") != Some(session.to_string().as_str())
            })
            || number(item, "deletionEpoch").is_none_or(|epoch| epoch == 0)
        {
            return Err(SessionObservationDeletionError::Corrupt {
                detail: "the deletion row identity disagrees with its key".to_owned(),
            });
        }
        match string(item, "operationId") {
            None => {
                return Err(SessionObservationDeletionError::Corrupt {
                    detail: "the deletion row has no coordinating operation".to_owned(),
                });
            }
            Some(found) if found != operation.to_string() => {
                return Err(SessionObservationDeletionError::Conflict);
            }
            Some(_) => {}
        }
        string(item, "state")
            .and_then(SessionObservationDeletionStatus::parse)
            .ok_or_else(|| SessionObservationDeletionError::Corrupt {
                detail: "the deletion row has no recognized durable state".to_owned(),
            })
    }

    fn shard(&self, scope: ScopeKey) -> u8 {
        let hash = scope.hash8();
        let prefix = u16::from_str_radix(&hash[..2], 16).unwrap_or(0);
        u8::try_from(prefix % u16::from(self.duty_shards)).unwrap_or(0)
    }
}

#[async_trait::async_trait]
impl SessionObservationDeletion for SessionObservationDeletionStore {
    async fn request(
        &self,
        request: SessionObservationDeletionRequest,
    ) -> Result<SessionObservationDeletionOutcome, SessionObservationDeletionError> {
        let scope = Self::scope(request.workspace, request.session);
        let shard = self.shard(scope);
        let outcome = self
            .dynamodb
            .update_item()
            .table_name(&self.table)
            .key(PK, AttributeValue::S(keys::frontier_pk(&scope)))
            .key(SK, AttributeValue::S(keys::DELETION_SK.to_owned()))
            .update_expression(
                "SET itemType = :item, scopeKey = :scope, workspaceId = :workspace, \
                 sessionId = :session, operationId = :operation, #state = :deleting, \
                 #epoch = if_not_exists(#epoch, :zero) + :one, \
                 requestedAt = :now, stateChangedAt = :now, \
                 attempts = :zero, cPk = :control, cSk = :due",
            )
            .condition_expression("attribute_not_exists(pk) OR #state = :none")
            .expression_attribute_names("#state", "state")
            .expression_attribute_names("#epoch", "deletionEpoch")
            .expression_attribute_values(":item", AttributeValue::S("scope_deletion".to_owned()))
            .expression_attribute_values(":scope", AttributeValue::S(scope.to_key()))
            .expression_attribute_values(
                ":workspace",
                AttributeValue::S(request.workspace.to_string()),
            )
            .expression_attribute_values(":session", AttributeValue::S(request.session.to_string()))
            .expression_attribute_values(
                ":operation",
                AttributeValue::S(request.operation.to_string()),
            )
            .expression_attribute_values(
                ":deleting",
                AttributeValue::S(DeletionState::Deleting.as_str().to_owned()),
            )
            .expression_attribute_values(
                ":none",
                AttributeValue::S(DeletionState::None.as_str().to_owned()),
            )
            .expression_attribute_values(":one", AttributeValue::N("1".to_owned()))
            .expression_attribute_values(":now", AttributeValue::S(request.now.to_wire()))
            .expression_attribute_values(":zero", AttributeValue::N("0".to_owned()))
            .expression_attribute_values(
                ":control",
                AttributeValue::S(keys::control_pk(ControlDomain::DeletionExecute, shard)),
            )
            .expression_attribute_values(
                ":due",
                AttributeValue::S(keys::control_sk(request.now, &request.session.to_string())),
            )
            .send()
            .await;
        if outcome.is_ok() {
            return Ok(SessionObservationDeletionOutcome::Started);
        }

        // Conditional loss and an ambiguous provider response are both
        // resolved from the durable identity. We never issue a blind second
        // increment/enqueue.
        if let Some(item) = self.load(scope).await? {
            return Self::resolve(&item, scope, request.operation)
                .map(SessionObservationDeletionOutcome::Replay);
        }
        let error = outcome.expect_err("the successful response returned above");
        Err(classify(&error, Idempotence::Write(Resolution::TargetItem)).into())
    }

    async fn status(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        operation: OperationId,
    ) -> Result<SessionObservationDeletionStatus, SessionObservationDeletionError> {
        let scope = Self::scope(workspace, session);
        let item = self
            .load(scope)
            .await?
            .ok_or(SessionObservationDeletionError::NotFound)?;
        Self::resolve(&item, scope, operation)
    }
}

fn string<'a>(item: &'a HashMap<String, AttributeValue>, name: &str) -> Option<&'a str> {
    match item.get(name) {
        Some(AttributeValue::S(value)) => Some(value),
        _ => None,
    }
}

fn number(item: &HashMap<String, AttributeValue>, name: &str) -> Option<u64> {
    match item.get(name) {
        Some(AttributeValue::N(value)) => value.parse().ok(),
        _ => None,
    }
}
