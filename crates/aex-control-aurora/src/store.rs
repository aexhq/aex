//! The Aurora implementation of the coarse central-control authority.

use async_trait::async_trait;
use time::OffsetDateTime;
use uuid::Uuid;

use aex_control_app::ports::{
    AcceptInvitationsTx, BeginWorkspaceDeletionTx, BeginWorkspaceProvisionTx, ClaimDueOperations,
    ClaimOutbox, CompleteWorkspaceDeletionTx, ControlStore, CreateApiKeyTx, CreateInvitationTx,
    CreateOrganizationTx, FinishWorkspaceProvisionTx, GcExpired, GcReport, IdempotencyRecordKey,
    ListApiKeys, ListOperations, ListOrganizations, ListWorkspaces, Page, PageRequest,
    ReconcileIdentity, RevokeApiKeyTx, StoreError, TxOutcome, UnknownCommit,
};
use aex_control_domain::{
    ApiKey, AuditEvent, Fence, Invitation, InvitationStatus, Membership, MembershipStatus,
    Operation, OperationKind, OperationStatus, OperationVisibility, OrgRole, Organization,
    OrganizationStatus, OutboxMessage, Revision, ScopeSet, Workspace, WorkspaceStatus,
};
use aex_rds_data::{DataApiClient, Isolation, SqlValue, Statement, Transaction};

use crate::error::{map_commit_failure, map_store_error};
use crate::rows::{
    ApiKeyRow, IdempotencyRow, InvitationRow, MembershipRow, OperationRow, OrganizationRow,
    OutboxRow, UuidRow, WorkspaceRow,
};
use crate::sql;

macro_rules! tx_try {
    ($transaction:ident, $future:expr) => {
        match $future.await {
            Ok(value) => value,
            Err(error) => {
                let mapped = map_store_error(error);
                let _ = $transaction.rollback().await;
                return Err(mapped);
            }
        }
    };
}

/// The central-control repository over the one Data API transport.
#[derive(Debug, Clone)]
pub struct AuroraControlStore {
    client: DataApiClient,
}

enum Replay {
    Fresh,
    InFlight(Option<Uuid>),
    Completed(serde_json::Value, Option<Uuid>),
    Conflict,
}

impl AuroraControlStore {
    /// Builds the control authority.
    #[must_use]
    pub const fn new(client: DataApiClient) -> Self {
        Self { client }
    }

    fn millis(instant: OffsetDateTime) -> i64 {
        i64::try_from(instant.unix_timestamp_nanos().div_euclid(1_000_000)).unwrap_or(i64::MAX)
    }

    fn optional_uuid(value: Option<Uuid>) -> SqlValue {
        value.map_or(SqlValue::Null, SqlValue::Uuid)
    }

    fn optional_i64(value: Option<u64>) -> SqlValue {
        value
            .and_then(|value| i64::try_from(value).ok())
            .map_or(SqlValue::Null, SqlValue::I64)
    }

    fn page_limit(page: &PageRequest) -> Result<i64, StoreError> {
        if page.cursor.is_some() {
            return Err(StoreError::Decode(
                "a signed cursor must be decoded before the Aurora port".to_owned(),
            ));
        }
        let limit = page.limit.min(aex_control_app::ports::MAX_PAGE_LIMIT);
        Ok(i64::from(limit))
    }

    fn required_scope(spelling: &str) -> ScopeSet {
        ScopeSet::from_strings(&[spelling])
            .expect("a source-owned operation scope is in the generated registry")
    }

    fn bind_idempotency<'a>(
        statement: Statement<'a>,
        key: &'a IdempotencyRecordKey,
    ) -> Statement<'a> {
        statement
            .bind("key_kind", SqlValue::Text(key.key_kind.as_str().to_owned()))
            .bind("key_value", SqlValue::Text(key.key_value.clone()))
            .bind(
                "principal_kind",
                SqlValue::Text(key.principal_kind.as_str().to_owned()),
            )
            .bind("principal_id", SqlValue::Uuid(key.principal_id))
            .bind(
                "scope_kind",
                SqlValue::Text(key.scope_kind.as_str().to_owned()),
            )
            .bind("scope_id", SqlValue::Uuid(key.scope_id))
            .bind("method", SqlValue::Text(key.method.as_str().to_owned()))
            .bind("route", SqlValue::Text(key.route.clone()))
    }

    async fn replay(
        transaction: &mut Transaction<'_>,
        key: &IdempotencyRecordKey,
    ) -> Result<Replay, aex_rds_data::DataApiError> {
        let row = transaction
            .query_opt::<IdempotencyRow>(Self::bind_idempotency(
                Statement::new(sql::FIND_IDEMPOTENCY),
                key,
            ))
            .await?;
        Ok(match row {
            None => Replay::Fresh,
            Some(row) if row.intent_hash != key.intent_hash => Replay::Conflict,
            Some(row) if row.state == "completed" => Replay::Completed(
                row.response_body.unwrap_or(serde_json::Value::Null),
                row.operation_id,
            ),
            Some(row) => Replay::InFlight(row.operation_id),
        })
    }

    async fn insert_idempotency(
        transaction: &mut Transaction<'_>,
        key: &IdempotencyRecordKey,
        now: OffsetDateTime,
    ) -> Result<(), aex_rds_data::DataApiError> {
        let statement = Self::bind_idempotency(Statement::new(sql::INSERT_IDEMPOTENCY), key)
            .bind("id", SqlValue::Uuid(key.id))
            .bind(
                "intent_hash",
                SqlValue::Bytes(key.intent_hash.as_bytes().to_vec()),
            )
            .bind("now_ms", SqlValue::TimestampMillis(Self::millis(now)))
            .bind(
                "expires_at_ms",
                SqlValue::TimestampMillis(Self::millis(key.expires_at)),
            );
        transaction.execute(statement).await.map(|_| ())
    }

    async fn complete_idempotency(
        transaction: &mut Transaction<'_>,
        id: Uuid,
        status: i64,
        body: serde_json::Value,
        operation_id: Option<Uuid>,
        now: OffsetDateTime,
    ) -> Result<u64, aex_rds_data::DataApiError> {
        transaction
            .execute(
                Statement::new(sql::COMPLETE_IDEMPOTENCY)
                    .bind("id", SqlValue::Uuid(id))
                    .bind("response_status", SqlValue::I64(status))
                    .bind("response_body", SqlValue::Json(body))
                    .bind("operation_id", Self::optional_uuid(operation_id))
                    .bind("now_ms", SqlValue::TimestampMillis(Self::millis(now))),
            )
            .await
    }

    async fn insert_audit(
        transaction: &mut Transaction<'_>,
        audit: &AuditEvent,
    ) -> Result<(), aex_rds_data::DataApiError> {
        transaction
            .execute(
                Statement::new(sql::INSERT_AUDIT)
                    .bind("id", SqlValue::Uuid(audit.id))
                    .bind(
                        "organization_id",
                        Self::optional_uuid(audit.organization_id),
                    )
                    .bind("workspace_id", Self::optional_uuid(audit.workspace_id))
                    .bind(
                        "actor_kind",
                        SqlValue::Text(audit.actor_kind.as_str().to_owned()),
                    )
                    .bind("actor_id", Self::optional_uuid(audit.actor_id))
                    .bind("action", SqlValue::Text(audit.action.clone()))
                    .bind(
                        "resource_kind",
                        SqlValue::Text(audit.resource_kind.as_str().to_owned()),
                    )
                    .bind("resource_id", Self::optional_uuid(audit.resource_id))
                    .bind("outcome", SqlValue::Text(audit.outcome.as_str().to_owned()))
                    .bind("request_id", SqlValue::Text(audit.request_id.clone()))
                    .bind("operation_id", Self::optional_uuid(audit.operation_id))
                    .bind("detail", SqlValue::Json(audit.detail.clone()))
                    .bind(
                        "occurred_at_ms",
                        SqlValue::TimestampMillis(Self::millis(audit.occurred_at)),
                    ),
            )
            .await
            .map(|_| ())
    }

    async fn insert_outbox(
        transaction: &mut Transaction<'_>,
        message: &OutboxMessage,
    ) -> Result<(), aex_rds_data::DataApiError> {
        transaction
            .execute(
                Statement::new(sql::INSERT_OUTBOX)
                    .bind("id", SqlValue::Uuid(message.id))
                    .bind("topic", SqlValue::Text(message.topic.as_str().to_owned()))
                    .bind("dedupe_key", SqlValue::Text(message.dedupe_key.clone()))
                    .bind("group_key", SqlValue::Text(message.group_key.clone()))
                    .bind("payload", SqlValue::Json(message.payload.clone()))
                    .bind("attempts", SqlValue::I64(i64::from(message.attempts)))
                    .bind(
                        "available_at_ms",
                        SqlValue::TimestampMillis(Self::millis(message.available_at)),
                    )
                    .bind(
                        "created_at_ms",
                        SqlValue::TimestampMillis(Self::millis(message.created_at)),
                    ),
            )
            .await
            .map(|_| ())
    }

    async fn commit<T>(
        transaction: Transaction<'_>,
        ceremony: &'static str,
        id: Uuid,
        value: T,
    ) -> Result<TxOutcome<T>, StoreError> {
        match transaction.commit().await {
            Ok(_) => Ok(TxOutcome::Committed(value)),
            Err(failure) => match map_commit_failure(failure) {
                StoreError::Unknown => Ok(TxOutcome::Unknown(UnknownCommit {
                    identity: ReconcileIdentity { ceremony, id },
                })),
                error => Err(error),
            },
        }
    }

    fn replay_id(body: &serde_json::Value) -> Result<Uuid, StoreError> {
        body.get("resourceId")
            .and_then(serde_json::Value::as_str)
            .and_then(|text| Uuid::parse_str(text).ok())
            .ok_or_else(|| StoreError::Decode("a completed replay has no resourceId".to_owned()))
    }

    async fn organization_in(
        transaction: &mut Transaction<'_>,
        id: Uuid,
    ) -> Result<Option<Organization>, aex_rds_data::DataApiError> {
        transaction
            .query_opt::<OrganizationRow>(
                Statement::new(sql::GET_ORGANIZATION).bind("organization_id", SqlValue::Uuid(id)),
            )
            .await
            .map(|row| row.map(|row| row.0))
    }

    async fn workspace_in(
        transaction: &mut Transaction<'_>,
        id: Uuid,
    ) -> Result<Option<Workspace>, aex_rds_data::DataApiError> {
        transaction
            .query_opt::<WorkspaceRow>(
                Statement::new(sql::GET_WORKSPACE).bind("workspace_id", SqlValue::Uuid(id)),
            )
            .await
            .map(|row| row.map(|row| row.0))
    }

    async fn operation_in(
        transaction: &mut Transaction<'_>,
        id: Uuid,
    ) -> Result<Option<Operation>, aex_rds_data::DataApiError> {
        transaction
            .query_opt::<OperationRow>(
                Statement::new(sql::GET_OPERATION).bind("operation_id", SqlValue::Uuid(id)),
            )
            .await
            .map(|row| row.map(|row| row.0))
    }

    async fn api_key_in(
        transaction: &mut Transaction<'_>,
        id: Uuid,
    ) -> Result<Option<ApiKey>, aex_rds_data::DataApiError> {
        transaction
            .query_opt::<ApiKeyRow>(
                Statement::new(sql::GET_API_KEY).bind("key_id", SqlValue::Uuid(id)),
            )
            .await
            .map(|row| row.map(|row| row.0))
    }

    async fn membership_in(
        transaction: &mut Transaction<'_>,
        organization_id: Uuid,
        user_id: Uuid,
    ) -> Result<Option<Membership>, aex_rds_data::DataApiError> {
        transaction
            .query_opt::<MembershipRow>(
                Statement::new(sql::FIND_MEMBERSHIP)
                    .bind("organization_id", SqlValue::Uuid(organization_id))
                    .bind("user_id", SqlValue::Uuid(user_id)),
            )
            .await
            .map(|row| row.map(|row| row.0))
    }

    async fn insert_operation(
        transaction: &mut Transaction<'_>,
        id: Uuid,
        kind: OperationKind,
        organization_id: Uuid,
        workspace_id: Uuid,
        key: &IdempotencyRecordKey,
        scopes: ScopeSet,
        now: OffsetDateTime,
    ) -> Result<(), aex_rds_data::DataApiError> {
        transaction
            .execute(
                Statement::new(sql::INSERT_OPERATION)
                    .bind("id", SqlValue::Uuid(id))
                    .bind("kind", SqlValue::Text(kind.as_str().to_owned()))
                    .bind(
                        "visibility",
                        SqlValue::Text(kind.visibility().as_str().to_owned()),
                    )
                    .bind("organization_id", SqlValue::Uuid(organization_id))
                    .bind("workspace_id", SqlValue::Uuid(workspace_id))
                    .bind(
                        "principal_kind",
                        SqlValue::Text(key.principal_kind.as_str().to_owned()),
                    )
                    .bind("principal_id", SqlValue::Uuid(key.principal_id))
                    .bind("scopes", SqlValue::TextArray(scopes.to_strings()))
                    .bind(
                        "intent_hash",
                        SqlValue::Bytes(key.intent_hash.as_bytes().to_vec()),
                    )
                    .bind("now_ms", SqlValue::TimestampMillis(Self::millis(now))),
            )
            .await
            .map(|_| ())
    }

    async fn attach_operation(
        transaction: &mut Transaction<'_>,
        idempotency_id: Uuid,
        operation_id: Uuid,
    ) -> Result<u64, aex_rds_data::DataApiError> {
        transaction
            .execute(
                Statement::new(sql::ATTACH_IDEMPOTENCY_OPERATION)
                    .bind("id", SqlValue::Uuid(idempotency_id))
                    .bind("operation_id", SqlValue::Uuid(operation_id)),
            )
            .await
    }

    async fn replay_workspace_operation(
        transaction: &mut Transaction<'_>,
        operation_id: Option<Uuid>,
    ) -> Result<(Workspace, Operation), StoreError> {
        let operation_id = operation_id.ok_or_else(|| {
            StoreError::Decode("an in-flight workspace replay has no operation".to_owned())
        })?;
        let operation = Self::operation_in(transaction, operation_id)
            .await
            .map_err(map_store_error)?
            .ok_or_else(|| StoreError::Decode("a replay operation no longer exists".to_owned()))?;
        let workspace_id = operation.workspace_id.ok_or_else(|| {
            StoreError::Decode("a workspace operation has no workspace".to_owned())
        })?;
        let workspace = Self::workspace_in(transaction, workspace_id)
            .await
            .map_err(map_store_error)?
            .ok_or_else(|| StoreError::Decode("a replay workspace no longer exists".to_owned()))?;
        Ok((workspace, operation))
    }
}

#[async_trait]
impl ControlStore for AuroraControlStore {
    async fn create_organization(
        &self,
        command: &CreateOrganizationTx,
    ) -> Result<TxOutcome<Organization>, StoreError> {
        let mut transaction = self
            .client
            .begin(Isolation::Serializable)
            .await
            .map_err(map_store_error)?;
        match tx_try!(
            transaction,
            Self::replay(&mut transaction, &command.idempotency)
        ) {
            Replay::Conflict => {
                let _ = transaction.rollback().await;
                return Ok(TxOutcome::IntentConflict);
            }
            Replay::Completed(body, _) => {
                let id = Self::replay_id(&body)?;
                let value = tx_try!(transaction, Self::organization_in(&mut transaction, id))
                    .ok_or_else(|| {
                        StoreError::Decode("a replay organization no longer exists".to_owned())
                    })?;
                let _ = transaction.rollback().await;
                return Ok(TxOutcome::Replayed(value));
            }
            Replay::InFlight(_) => {
                let _ = transaction.rollback().await;
                return Err(StoreError::Decode(
                    "a non-operation organization replay remained in flight".to_owned(),
                ));
            }
            Replay::Fresh => {}
        }
        tx_try!(
            transaction,
            Self::insert_idempotency(&mut transaction, &command.idempotency, command.now)
        );
        tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::INSERT_ORGANIZATION)
                    .bind("id", SqlValue::Uuid(command.preassigned_id))
                    .bind("name", SqlValue::Text(command.name.clone()))
                    .bind("slug", SqlValue::Text(command.slug.as_str().to_owned()))
                    .bind(
                        "created_by_user_id",
                        SqlValue::Uuid(command.created_by_user_id),
                    )
                    .bind(
                        "now_ms",
                        SqlValue::TimestampMillis(Self::millis(command.now)),
                    ),
            )
        );
        tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::INSERT_MEMBERSHIP)
                    .bind("id", SqlValue::Uuid(command.preassigned_membership_id))
                    .bind("organization_id", SqlValue::Uuid(command.preassigned_id))
                    .bind("user_id", SqlValue::Uuid(command.created_by_user_id))
                    .bind("role", SqlValue::Text(OrgRole::Owner.as_str().to_owned()))
                    .bind(
                        "now_ms",
                        SqlValue::TimestampMillis(Self::millis(command.now)),
                    ),
            )
        );
        tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::ENSURE_FINANCE_ACCOUNT)
                    .bind("organization_id", SqlValue::Uuid(command.preassigned_id)),
            )
        );
        tx_try!(
            transaction,
            Self::insert_audit(&mut transaction, &command.audit)
        );
        let completed = tx_try!(
            transaction,
            Self::complete_idempotency(
                &mut transaction,
                command.idempotency.id,
                201,
                serde_json::json!({ "resourceId": command.preassigned_id.to_string() }),
                None,
                command.now,
            )
        );
        if completed != 1 {
            let _ = transaction.rollback().await;
            return Err(StoreError::Conflict {
                constraint: "idempotency_in_flight".to_owned(),
            });
        }
        let organization = Organization {
            id: command.preassigned_id,
            name: command.name.clone(),
            slug: command.slug.clone(),
            status: OrganizationStatus::Active,
            revision: Revision::INITIAL,
            created_at: command.now,
            updated_at: command.now,
            created_by_user_id: command.created_by_user_id,
        };
        Self::commit(
            transaction,
            aex_control_app::use_cases::ceremony::CREATE_ORGANIZATION,
            command.preassigned_id,
            organization,
        )
        .await
    }

    async fn list_organizations(
        &self,
        query: &ListOrganizations,
    ) -> Result<Page<Organization>, StoreError> {
        let limit = Self::page_limit(&query.page)?;
        let rows = self
            .client
            .query::<OrganizationRow>(
                Statement::new(sql::LIST_ORGANIZATIONS)
                    .bind("user_id", SqlValue::Uuid(query.user_id))
                    .bind("limit", SqlValue::I64(limit)),
            )
            .await
            .map_err(map_store_error)?;
        Ok(Page {
            items: rows.into_iter().map(|row| row.0).collect(),
            next_cursor: None,
        })
    }

    async fn get_organization(&self, id: Uuid) -> Result<Option<Organization>, StoreError> {
        self.client
            .query_opt::<OrganizationRow>(
                Statement::new(sql::GET_ORGANIZATION).bind("organization_id", SqlValue::Uuid(id)),
            )
            .await
            .map(|row| row.map(|row| row.0))
            .map_err(map_store_error)
    }

    async fn list_memberships(
        &self,
        organization_id: Uuid,
        page: &PageRequest,
    ) -> Result<Page<Membership>, StoreError> {
        let limit = Self::page_limit(page)?;
        let rows = self
            .client
            .query::<MembershipRow>(
                Statement::new(sql::LIST_MEMBERSHIPS)
                    .bind("organization_id", SqlValue::Uuid(organization_id))
                    .bind("limit", SqlValue::I64(limit)),
            )
            .await
            .map_err(map_store_error)?;
        Ok(Page {
            items: rows.into_iter().map(|row| row.0).collect(),
            next_cursor: None,
        })
    }

    async fn create_invitation(
        &self,
        command: &CreateInvitationTx,
    ) -> Result<TxOutcome<Invitation>, StoreError> {
        let mut transaction = self
            .client
            .begin(Isolation::Serializable)
            .await
            .map_err(map_store_error)?;
        match tx_try!(
            transaction,
            Self::replay(&mut transaction, &command.idempotency)
        ) {
            Replay::Conflict => {
                let _ = transaction.rollback().await;
                return Ok(TxOutcome::IntentConflict);
            }
            Replay::Completed(body, _) => {
                let id = Self::replay_id(&body)?;
                let value = tx_try!(
                    transaction,
                    transaction.query_opt::<InvitationRow>(
                        Statement::new(sql::GET_INVITATION)
                            .bind("invitation_id", SqlValue::Uuid(id)),
                    )
                )
                .map(|row| row.0)
                .ok_or_else(|| {
                    StoreError::Decode("a replay invitation no longer exists".to_owned())
                })?;
                let _ = transaction.rollback().await;
                return Ok(TxOutcome::Replayed(value));
            }
            Replay::InFlight(_) => {
                let _ = transaction.rollback().await;
                return Err(StoreError::Decode(
                    "a non-operation invitation replay remained in flight".to_owned(),
                ));
            }
            Replay::Fresh => {}
        }
        tx_try!(
            transaction,
            Self::insert_idempotency(&mut transaction, &command.idempotency, command.now)
        );
        tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::INSERT_INVITATION)
                    .bind("id", SqlValue::Uuid(command.preassigned_id))
                    .bind("organization_id", SqlValue::Uuid(command.organization_id))
                    .bind("email", SqlValue::Text(command.email.clone()))
                    .bind("role", SqlValue::Text(command.role.as_str().to_owned()))
                    .bind(
                        "invited_by_user_id",
                        SqlValue::Uuid(command.invited_by_user_id),
                    )
                    .bind(
                        "now_ms",
                        SqlValue::TimestampMillis(Self::millis(command.now)),
                    )
                    .bind(
                        "expires_at_ms",
                        SqlValue::TimestampMillis(Self::millis(command.expires_at)),
                    ),
            )
        );
        tx_try!(
            transaction,
            Self::insert_outbox(&mut transaction, &command.outbox)
        );
        tx_try!(
            transaction,
            Self::insert_audit(&mut transaction, &command.audit)
        );
        let completed = tx_try!(
            transaction,
            Self::complete_idempotency(
                &mut transaction,
                command.idempotency.id,
                201,
                serde_json::json!({ "resourceId": command.preassigned_id.to_string() }),
                None,
                command.now,
            )
        );
        if completed != 1 {
            let _ = transaction.rollback().await;
            return Err(StoreError::Conflict {
                constraint: "idempotency_in_flight".to_owned(),
            });
        }
        let invitation = Invitation {
            id: command.preassigned_id,
            organization_id: command.organization_id,
            email: command.email.clone(),
            role: command.role,
            status: InvitationStatus::Pending,
            invited_by_user_id: command.invited_by_user_id,
            accepted_user_id: None,
            created_at: command.now,
            expires_at: command.expires_at,
            resolved_at: None,
        };
        Self::commit(
            transaction,
            aex_control_app::use_cases::ceremony::CREATE_INVITATION,
            command.preassigned_id,
            invitation,
        )
        .await
    }

    async fn accept_invitations_for_email(
        &self,
        command: &AcceptInvitationsTx,
    ) -> Result<TxOutcome<Vec<Membership>>, StoreError> {
        if !command.email_verified {
            return Ok(TxOutcome::Replayed(Vec::new()));
        }
        let mut transaction = self
            .client
            .begin(Isolation::Serializable)
            .await
            .map_err(map_store_error)?;
        let invitations = tx_try!(
            transaction,
            transaction.query::<InvitationRow>(
                Statement::new(sql::FIND_ACCEPTABLE_INVITATIONS)
                    .bind("email", SqlValue::Text(command.email.clone()))
                    .bind(
                        "now_ms",
                        SqlValue::TimestampMillis(Self::millis(command.now)),
                    ),
            )
        );
        if invitations.is_empty() {
            let _ = transaction.rollback().await;
            return Ok(TxOutcome::Replayed(Vec::new()));
        }
        if command.preassigned_membership_ids.len() != invitations.len() {
            let _ = transaction.rollback().await;
            return Err(StoreError::Fatal(
                "invitation acceptance needs one preassigned membership id per row".to_owned(),
            ));
        }
        let mut memberships = Vec::with_capacity(invitations.len());
        for (invitation, membership_id) in invitations
            .into_iter()
            .map(|row| row.0)
            .zip(command.preassigned_membership_ids.iter().copied())
        {
            let membership = tx_try!(
                transaction,
                Self::membership_in(
                    &mut transaction,
                    invitation.organization_id,
                    command.user_id,
                )
            );
            let membership = match membership {
                Some(existing) => {
                    let raised = existing.raise_role_to(invitation.role, command.now);
                    if raised != existing {
                        tx_try!(
                            transaction,
                            transaction.execute(
                                Statement::new(sql::RAISE_MEMBERSHIP_ROLE)
                                    .bind("membership_id", SqlValue::Uuid(existing.id))
                                    .bind(
                                        "role",
                                        SqlValue::Text(invitation.role.as_str().to_owned()),
                                    )
                                    .bind(
                                        "now_ms",
                                        SqlValue::TimestampMillis(Self::millis(command.now)),
                                    ),
                            )
                        );
                    }
                    raised
                }
                None => {
                    tx_try!(
                        transaction,
                        transaction.execute(
                            Statement::new(sql::INSERT_MEMBERSHIP)
                                .bind("id", SqlValue::Uuid(membership_id))
                                .bind(
                                    "organization_id",
                                    SqlValue::Uuid(invitation.organization_id),
                                )
                                .bind("user_id", SqlValue::Uuid(command.user_id))
                                .bind("role", SqlValue::Text(invitation.role.as_str().to_owned()),)
                                .bind(
                                    "now_ms",
                                    SqlValue::TimestampMillis(Self::millis(command.now)),
                                ),
                        )
                    );
                    Membership {
                        id: membership_id,
                        organization_id: invitation.organization_id,
                        user_id: command.user_id,
                        role: invitation.role,
                        status: MembershipStatus::Active,
                        revision: Revision::INITIAL,
                        created_at: command.now,
                        updated_at: command.now,
                    }
                }
            };
            let accepted = tx_try!(
                transaction,
                transaction.execute(
                    Statement::new(sql::ACCEPT_INVITATION)
                        .bind("invitation_id", SqlValue::Uuid(invitation.id))
                        .bind("user_id", SqlValue::Uuid(command.user_id))
                        .bind(
                            "now_ms",
                            SqlValue::TimestampMillis(Self::millis(command.now)),
                        ),
                )
            );
            if accepted != 1 {
                let _ = transaction.rollback().await;
                return Err(StoreError::Conflict {
                    constraint: "invitation_pending".to_owned(),
                });
            }
            memberships.push(membership);
        }
        Self::commit(
            transaction,
            "control.accept_invitations",
            command.user_id,
            memberships,
        )
        .await
    }

    async fn begin_workspace_provision(
        &self,
        command: &BeginWorkspaceProvisionTx,
    ) -> Result<TxOutcome<(Workspace, Operation)>, StoreError> {
        let mut transaction = self
            .client
            .begin(Isolation::Serializable)
            .await
            .map_err(map_store_error)?;
        match tx_try!(
            transaction,
            Self::replay(&mut transaction, &command.idempotency)
        ) {
            Replay::Conflict => {
                let _ = transaction.rollback().await;
                return Ok(TxOutcome::IntentConflict);
            }
            Replay::InFlight(operation_id) | Replay::Completed(_, operation_id) => {
                let pair =
                    match Self::replay_workspace_operation(&mut transaction, operation_id).await {
                        Ok(pair) => pair,
                        Err(error) => {
                            let _ = transaction.rollback().await;
                            return Err(error);
                        }
                    };
                let _ = transaction.rollback().await;
                return Ok(TxOutcome::Replayed(pair));
            }
            Replay::Fresh => {}
        }
        tx_try!(
            transaction,
            Self::insert_idempotency(&mut transaction, &command.idempotency, command.now)
        );
        tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::INSERT_WORKSPACE)
                    .bind("id", SqlValue::Uuid(command.preassigned_workspace_id))
                    .bind("organization_id", SqlValue::Uuid(command.organization_id))
                    .bind("name", SqlValue::Text(command.name.clone()))
                    .bind("slug", SqlValue::Text(command.slug.as_str().to_owned()))
                    .bind("region", SqlValue::Text(command.region.as_str().to_owned()))
                    .bind(
                        "operation_id",
                        SqlValue::Uuid(command.preassigned_operation_id),
                    )
                    .bind(
                        "created_by_user_id",
                        SqlValue::Uuid(command.created_by_user_id),
                    )
                    .bind(
                        "now_ms",
                        SqlValue::TimestampMillis(Self::millis(command.now)),
                    ),
            )
        );
        let scopes = Self::required_scope("workspaces:write");
        tx_try!(
            transaction,
            Self::insert_operation(
                &mut transaction,
                command.preassigned_operation_id,
                OperationKind::WorkspaceProvision,
                command.organization_id,
                command.preassigned_workspace_id,
                &command.idempotency,
                scopes,
                command.now,
            )
        );
        let attached = tx_try!(
            transaction,
            Self::attach_operation(
                &mut transaction,
                command.idempotency.id,
                command.preassigned_operation_id,
            )
        );
        if attached != 1 {
            let _ = transaction.rollback().await;
            return Err(StoreError::Conflict {
                constraint: "idempotency_operation_unattached".to_owned(),
            });
        }
        tx_try!(
            transaction,
            Self::insert_outbox(&mut transaction, &command.outbox)
        );
        tx_try!(
            transaction,
            Self::insert_audit(&mut transaction, &command.audit)
        );
        let workspace = Workspace {
            id: command.preassigned_workspace_id,
            organization_id: command.organization_id,
            name: command.name.clone(),
            slug: command.slug.clone(),
            region: command.region,
            status: WorkspaceStatus::Provisioning,
            provision_operation_id: command.preassigned_operation_id,
            provision_fence: Fence::FIRST,
            deletion_operation_id: None,
            deletion_fence: None,
            revision: Revision::INITIAL,
            created_at: command.now,
            updated_at: command.now,
            activated_at: None,
            deleted_at: None,
            created_by_user_id: command.created_by_user_id,
        };
        let operation = Operation {
            id: command.preassigned_operation_id,
            kind: OperationKind::WorkspaceProvision,
            visibility: OperationVisibility::Internal,
            organization_id: command.organization_id,
            workspace_id: Some(command.preassigned_workspace_id),
            principal_id: command.idempotency.principal_id,
            scopes,
            status: OperationStatus::Queued,
            intent_hash: command.idempotency.intent_hash,
            fence: Fence::FIRST,
            attempt: 0,
            lease: None,
            created_at: command.now,
            started_at: None,
            updated_at: command.now,
            terminal_at: None,
            due_at: Some(command.now),
        };
        Self::commit(
            transaction,
            aex_control_app::use_cases::ceremony::BEGIN_WORKSPACE_PROVISION,
            command.preassigned_workspace_id,
            (workspace, operation),
        )
        .await
    }

    async fn finish_workspace_provision(
        &self,
        command: &FinishWorkspaceProvisionTx,
    ) -> Result<TxOutcome<Workspace>, StoreError> {
        let mut transaction = self
            .client
            .begin(Isolation::Serializable)
            .await
            .map_err(map_store_error)?;
        let before = tx_try!(
            transaction,
            Self::workspace_in(&mut transaction, command.workspace_id)
        )
        .ok_or(StoreError::NotFound)?;
        if before.status == WorkspaceStatus::Active
            && before.provision_operation_id == command.operation_id
            && before.provision_fence.get() >= command.fence.get()
        {
            let _ = transaction.rollback().await;
            return Ok(TxOutcome::Replayed(before));
        }
        let updated = tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::FINISH_WORKSPACE_PROVISION)
                    .bind("workspace_id", SqlValue::Uuid(command.workspace_id))
                    .bind("operation_id", SqlValue::Uuid(command.operation_id))
                    .bind(
                        "fence",
                        SqlValue::I64(i64::try_from(command.fence.get()).unwrap_or(i64::MAX)),
                    )
                    .bind(
                        "now_ms",
                        SqlValue::TimestampMillis(Self::millis(command.now)),
                    ),
            )
        );
        if updated != 1 {
            let _ = transaction.rollback().await;
            return Err(StoreError::Conflict {
                constraint: "workspace_provision_fence".to_owned(),
            });
        }
        let succeeded = tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::SUCCEED_OPERATION)
                    .bind("operation_id", SqlValue::Uuid(command.operation_id))
                    .bind(
                        "fence",
                        SqlValue::I64(i64::try_from(command.fence.get()).unwrap_or(i64::MAX)),
                    )
                    .bind("result", SqlValue::Json(command.response_body.clone()))
                    .bind(
                        "now_ms",
                        SqlValue::TimestampMillis(Self::millis(command.now)),
                    ),
            )
        );
        if succeeded != 1 {
            let _ = transaction.rollback().await;
            return Err(StoreError::Conflict {
                constraint: "operation_fence".to_owned(),
            });
        }
        let completed = tx_try!(
            transaction,
            Self::complete_idempotency(
                &mut transaction,
                command.idempotency_id,
                201,
                command.response_body.clone(),
                Some(command.operation_id),
                command.now,
            )
        );
        if completed != 1 {
            let _ = transaction.rollback().await;
            return Err(StoreError::Conflict {
                constraint: "idempotency_in_flight".to_owned(),
            });
        }
        tx_try!(
            transaction,
            Self::insert_audit(&mut transaction, &command.audit)
        );
        let workspace = tx_try!(
            transaction,
            Self::workspace_in(&mut transaction, command.workspace_id)
        )
        .ok_or(StoreError::NotFound)?;
        Self::commit(
            transaction,
            aex_control_app::use_cases::ceremony::FINISH_WORKSPACE_PROVISION,
            command.workspace_id,
            workspace,
        )
        .await
    }

    async fn list_workspaces(&self, query: &ListWorkspaces) -> Result<Page<Workspace>, StoreError> {
        let limit = Self::page_limit(&query.page)?;
        let rows = self
            .client
            .query::<WorkspaceRow>(
                Statement::new(sql::LIST_WORKSPACES)
                    .bind("user_id", SqlValue::Uuid(query.user_id))
                    .bind(
                        "organization_id",
                        Self::optional_uuid(query.organization_id),
                    )
                    .bind("limit", SqlValue::I64(limit)),
            )
            .await
            .map_err(map_store_error)?;
        Ok(Page {
            items: rows.into_iter().map(|row| row.0).collect(),
            next_cursor: None,
        })
    }

    async fn get_workspace(&self, id: Uuid) -> Result<Option<Workspace>, StoreError> {
        self.client
            .query_opt::<WorkspaceRow>(
                Statement::new(sql::GET_WORKSPACE).bind("workspace_id", SqlValue::Uuid(id)),
            )
            .await
            .map(|row| row.map(|row| row.0))
            .map_err(map_store_error)
    }

    async fn begin_workspace_deletion(
        &self,
        command: &BeginWorkspaceDeletionTx,
    ) -> Result<TxOutcome<(Workspace, Operation)>, StoreError> {
        let mut transaction = self
            .client
            .begin(Isolation::Serializable)
            .await
            .map_err(map_store_error)?;
        match tx_try!(
            transaction,
            Self::replay(&mut transaction, &command.idempotency)
        ) {
            Replay::Conflict => {
                let _ = transaction.rollback().await;
                return Ok(TxOutcome::IntentConflict);
            }
            Replay::InFlight(operation_id) | Replay::Completed(_, operation_id) => {
                let pair =
                    match Self::replay_workspace_operation(&mut transaction, operation_id).await {
                        Ok(pair) => pair,
                        Err(error) => {
                            let _ = transaction.rollback().await;
                            return Err(error);
                        }
                    };
                let _ = transaction.rollback().await;
                return Ok(TxOutcome::Replayed(pair));
            }
            Replay::Fresh => {}
        }
        tx_try!(
            transaction,
            Self::insert_idempotency(&mut transaction, &command.idempotency, command.now)
        );
        let updated = tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::BEGIN_WORKSPACE_DELETION)
                    .bind("workspace_id", SqlValue::Uuid(command.workspace_id))
                    .bind("organization_id", SqlValue::Uuid(command.organization_id))
                    .bind("operation_id", SqlValue::Uuid(command.operation_id))
                    .bind(
                        "now_ms",
                        SqlValue::TimestampMillis(Self::millis(command.now)),
                    ),
            )
        );
        if updated != 1 {
            let _ = transaction.rollback().await;
            return Err(StoreError::Conflict {
                constraint: "workspace_active".to_owned(),
            });
        }
        let revoked = tx_try!(
            transaction,
            transaction.query::<UuidRow>(
                Statement::new(sql::REVOKE_WORKSPACE_KEYS)
                    .bind("workspace_id", SqlValue::Uuid(command.workspace_id))
                    .bind(
                        "now_ms",
                        SqlValue::TimestampMillis(Self::millis(command.now)),
                    ),
            )
        );
        for key in revoked {
            tx_try!(
                transaction,
                transaction.execute(
                    Statement::new(sql::BUMP_KEY_EPOCH).bind("key_id", SqlValue::Uuid(key.0)),
                )
            );
        }
        tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::BUMP_WORKSPACE_EPOCH)
                    .bind("workspace_id", SqlValue::Uuid(command.workspace_id)),
            )
        );
        let scopes = Self::required_scope("workspaces:delete");
        tx_try!(
            transaction,
            Self::insert_operation(
                &mut transaction,
                command.operation_id,
                OperationKind::WorkspaceDelete,
                command.organization_id,
                command.workspace_id,
                &command.idempotency,
                scopes,
                command.now,
            )
        );
        let attached = tx_try!(
            transaction,
            Self::attach_operation(
                &mut transaction,
                command.idempotency.id,
                command.operation_id,
            )
        );
        if attached != 1 {
            let _ = transaction.rollback().await;
            return Err(StoreError::Conflict {
                constraint: "idempotency_operation_unattached".to_owned(),
            });
        }
        tx_try!(
            transaction,
            Self::insert_outbox(&mut transaction, &command.outbox)
        );
        tx_try!(
            transaction,
            Self::insert_audit(&mut transaction, &command.audit)
        );
        let pair = (
            tx_try!(
                transaction,
                Self::workspace_in(&mut transaction, command.workspace_id)
            )
            .ok_or(StoreError::NotFound)?,
            tx_try!(
                transaction,
                Self::operation_in(&mut transaction, command.operation_id)
            )
            .ok_or(StoreError::NotFound)?,
        );
        Self::commit(
            transaction,
            aex_control_app::use_cases::ceremony::BEGIN_WORKSPACE_DELETION,
            command.operation_id,
            pair,
        )
        .await
    }

    async fn complete_workspace_deletion(
        &self,
        command: &CompleteWorkspaceDeletionTx,
    ) -> Result<TxOutcome<Workspace>, StoreError> {
        let mut transaction = self
            .client
            .begin(Isolation::Serializable)
            .await
            .map_err(map_store_error)?;
        let before = tx_try!(
            transaction,
            Self::workspace_in(&mut transaction, command.workspace_id)
        )
        .ok_or(StoreError::NotFound)?;
        if before.status == WorkspaceStatus::Deleted
            && before.deletion_operation_id == Some(command.operation_id)
            && before
                .deletion_fence
                .is_some_and(|fence| fence.get() >= command.fence.get())
        {
            let _ = transaction.rollback().await;
            return Ok(TxOutcome::Replayed(before));
        }
        let updated = tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::COMPLETE_WORKSPACE_DELETION)
                    .bind("workspace_id", SqlValue::Uuid(command.workspace_id))
                    .bind("operation_id", SqlValue::Uuid(command.operation_id))
                    .bind(
                        "fence",
                        SqlValue::I64(i64::try_from(command.fence.get()).unwrap_or(i64::MAX)),
                    )
                    .bind(
                        "now_ms",
                        SqlValue::TimestampMillis(Self::millis(command.now)),
                    ),
            )
        );
        if updated != 1 {
            let _ = transaction.rollback().await;
            return Err(StoreError::Conflict {
                constraint: "workspace_deletion_fence".to_owned(),
            });
        }
        let succeeded = tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::SUCCEED_OPERATION)
                    .bind("operation_id", SqlValue::Uuid(command.operation_id))
                    .bind(
                        "fence",
                        SqlValue::I64(i64::try_from(command.fence.get()).unwrap_or(i64::MAX)),
                    )
                    .bind(
                        "result",
                        SqlValue::Json(serde_json::json!({
                            "workspaceId": command.workspace_id.to_string(),
                            "deleted": true,
                        })),
                    )
                    .bind(
                        "now_ms",
                        SqlValue::TimestampMillis(Self::millis(command.now)),
                    ),
            )
        );
        if succeeded != 1 {
            let _ = transaction.rollback().await;
            return Err(StoreError::Conflict {
                constraint: "operation_fence".to_owned(),
            });
        }
        tx_try!(
            transaction,
            Self::insert_audit(&mut transaction, &command.audit)
        );
        let workspace = tx_try!(
            transaction,
            Self::workspace_in(&mut transaction, command.workspace_id)
        )
        .ok_or(StoreError::NotFound)?;
        Self::commit(
            transaction,
            "control.complete_workspace_deletion",
            command.operation_id,
            workspace,
        )
        .await
    }

    async fn create_api_key(
        &self,
        command: &CreateApiKeyTx,
    ) -> Result<TxOutcome<ApiKey>, StoreError> {
        let mut transaction = self
            .client
            .begin(Isolation::Serializable)
            .await
            .map_err(map_store_error)?;
        match tx_try!(
            transaction,
            Self::replay(&mut transaction, &command.idempotency)
        ) {
            Replay::Conflict => {
                let _ = transaction.rollback().await;
                return Ok(TxOutcome::IntentConflict);
            }
            Replay::Completed(body, _) => {
                let id = Self::replay_id(&body)?;
                let value = tx_try!(transaction, Self::api_key_in(&mut transaction, id))
                    .ok_or_else(|| {
                        StoreError::Decode("a replay API key no longer exists".to_owned())
                    })?;
                let _ = transaction.rollback().await;
                return Ok(TxOutcome::Replayed(value));
            }
            Replay::InFlight(_) => {
                let _ = transaction.rollback().await;
                return Err(StoreError::Decode(
                    "a non-operation API-key replay remained in flight".to_owned(),
                ));
            }
            Replay::Fresh => {}
        }
        let workspace = tx_try!(
            transaction,
            Self::workspace_in(&mut transaction, command.workspace_id)
        )
        .ok_or(StoreError::NotFound)?;
        if workspace.organization_id != command.organization_id {
            let _ = transaction.rollback().await;
            return Err(StoreError::Conflict {
                constraint: "key_workspace_organization_fk".to_owned(),
            });
        }
        if workspace.status != WorkspaceStatus::Active {
            let _ = transaction.rollback().await;
            return Err(StoreError::Conflict {
                constraint: "workspace_active".to_owned(),
            });
        }
        if workspace.region != command.region {
            let _ = transaction.rollback().await;
            return Err(StoreError::Conflict {
                constraint: "key_workspace_region".to_owned(),
            });
        }
        tx_try!(
            transaction,
            Self::insert_idempotency(&mut transaction, &command.idempotency, command.now)
        );
        tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::INSERT_API_KEY)
                    .bind("id", SqlValue::Uuid(command.preassigned_id))
                    .bind("workspace_id", SqlValue::Uuid(command.workspace_id))
                    .bind("organization_id", SqlValue::Uuid(command.organization_id))
                    .bind("name", SqlValue::Text(command.name.clone()))
                    .bind("scopes", SqlValue::TextArray(command.scopes.to_strings()))
                    .bind("region", SqlValue::Text(command.region.as_str().to_owned()))
                    .bind("verifier", SqlValue::Bytes(command.verifier.to_vec()))
                    .bind(
                        "pepper_version",
                        SqlValue::I64(i64::from(command.pepper_version)),
                    )
                    .bind(
                        "created_by_user_id",
                        SqlValue::Uuid(command.created_by_user_id),
                    )
                    .bind(
                        "now_ms",
                        SqlValue::TimestampMillis(Self::millis(command.now)),
                    ),
            )
        );
        tx_try!(
            transaction,
            Self::insert_audit(&mut transaction, &command.audit)
        );
        let completed = tx_try!(
            transaction,
            Self::complete_idempotency(
                &mut transaction,
                command.idempotency.id,
                201,
                serde_json::json!({ "resourceId": command.preassigned_id.to_string() }),
                None,
                command.now,
            )
        );
        if completed != 1 {
            let _ = transaction.rollback().await;
            return Err(StoreError::Conflict {
                constraint: "idempotency_in_flight".to_owned(),
            });
        }
        let key = ApiKey {
            id: command.preassigned_id,
            workspace_id: command.workspace_id,
            organization_id: command.organization_id,
            name: command.name.clone(),
            scopes: command.scopes,
            region: command.region,
            pepper_version: command.pepper_version,
            created_at: command.now,
            revoked_at: None,
            revision: Revision::INITIAL,
            created_by_user_id: command.created_by_user_id,
        };
        Self::commit(
            transaction,
            aex_control_app::use_cases::ceremony::CREATE_API_KEY,
            command.preassigned_id,
            key,
        )
        .await
    }

    async fn list_api_keys(&self, query: &ListApiKeys) -> Result<Page<ApiKey>, StoreError> {
        let limit = Self::page_limit(&query.page)?;
        let rows = self
            .client
            .query::<ApiKeyRow>(
                Statement::new(sql::LIST_API_KEYS)
                    .bind("workspace_id", SqlValue::Uuid(query.workspace_id))
                    .bind("limit", SqlValue::I64(limit)),
            )
            .await
            .map_err(map_store_error)?;
        Ok(Page {
            items: rows.into_iter().map(|row| row.0).collect(),
            next_cursor: None,
        })
    }

    async fn revoke_api_key(&self, command: &RevokeApiKeyTx) -> Result<TxOutcome<()>, StoreError> {
        let mut transaction = self
            .client
            .begin(Isolation::Serializable)
            .await
            .map_err(map_store_error)?;
        let key = tx_try!(
            transaction,
            Self::api_key_in(&mut transaction, command.key_id)
        )
        .filter(|key| key.workspace_id == command.workspace_id)
        .ok_or(StoreError::NotFound)?;
        if key.revoked_at.is_some() {
            let _ = transaction.rollback().await;
            return Ok(TxOutcome::Replayed(()));
        }
        if command
            .expected_revision
            .is_some_and(|expected| expected != key.revision.get())
        {
            let _ = transaction.rollback().await;
            return Err(StoreError::Conflict {
                constraint: "api_key_revision".to_owned(),
            });
        }
        let updated = tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::REVOKE_API_KEY)
                    .bind("key_id", SqlValue::Uuid(command.key_id))
                    .bind("workspace_id", SqlValue::Uuid(command.workspace_id))
                    .bind(
                        "expected_revision",
                        Self::optional_i64(command.expected_revision),
                    )
                    .bind(
                        "now_ms",
                        SqlValue::TimestampMillis(Self::millis(command.now)),
                    ),
            )
        );
        if updated != 1 {
            let _ = transaction.rollback().await;
            return Err(StoreError::Conflict {
                constraint: "api_key_revision".to_owned(),
            });
        }
        tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::BUMP_KEY_EPOCH).bind("key_id", SqlValue::Uuid(command.key_id)),
            )
        );
        tx_try!(
            transaction,
            Self::insert_audit(&mut transaction, &command.audit)
        );
        Self::commit(transaction, "control.revoke_api_key", command.key_id, ()).await
    }

    async fn get_operation(&self, id: Uuid) -> Result<Option<Operation>, StoreError> {
        self.client
            .query_opt::<OperationRow>(
                Statement::new(sql::GET_OPERATION).bind("operation_id", SqlValue::Uuid(id)),
            )
            .await
            .map(|row| row.map(|row| row.0))
            .map_err(map_store_error)
    }

    async fn list_operations(&self, query: &ListOperations) -> Result<Page<Operation>, StoreError> {
        let limit = Self::page_limit(&query.page)?;
        let rows = self
            .client
            .query::<OperationRow>(
                Statement::new(sql::LIST_OPERATIONS)
                    .bind("organization_id", SqlValue::Uuid(query.organization_id))
                    .bind("limit", SqlValue::I64(limit)),
            )
            .await
            .map_err(map_store_error)?;
        Ok(Page {
            items: rows.into_iter().map(|row| row.0).collect(),
            next_cursor: None,
        })
    }

    async fn claim_due_operations(
        &self,
        command: &ClaimDueOperations,
    ) -> Result<Vec<Operation>, StoreError> {
        let lease_expires_at = command.now.checked_add(command.lease).ok_or_else(|| {
            StoreError::Fatal("the operation lease exceeds the timestamp range".to_owned())
        })?;
        let mut transaction = self
            .client
            .begin(Isolation::ReadCommitted)
            .await
            .map_err(map_store_error)?;
        let rows = tx_try!(
            transaction,
            transaction.query::<OperationRow>(
                Statement::new(sql::CLAIM_DUE_OPERATIONS)
                    .bind("owner", SqlValue::Text(command.owner.clone()))
                    .bind(
                        "now_ms",
                        SqlValue::TimestampMillis(Self::millis(command.now)),
                    )
                    .bind(
                        "lease_expires_at_ms",
                        SqlValue::TimestampMillis(Self::millis(lease_expires_at)),
                    )
                    .bind("batch", SqlValue::I64(i64::from(command.batch))),
            )
        );
        transaction.commit().await.map_err(map_commit_failure)?;
        Ok(rows.into_iter().map(|row| row.0).collect())
    }

    async fn claim_outbox(&self, command: &ClaimOutbox) -> Result<Vec<OutboxMessage>, StoreError> {
        let lease_expires_at = command.now.checked_add(command.lease).ok_or_else(|| {
            StoreError::Fatal("the outbox lease exceeds the timestamp range".to_owned())
        })?;
        let mut transaction = self
            .client
            .begin(Isolation::ReadCommitted)
            .await
            .map_err(map_store_error)?;
        let rows = tx_try!(
            transaction,
            transaction.query::<OutboxRow>(
                Statement::new(sql::CLAIM_OUTBOX)
                    .bind("owner", SqlValue::Text(command.owner.clone()))
                    .bind(
                        "now_ms",
                        SqlValue::TimestampMillis(Self::millis(command.now)),
                    )
                    .bind(
                        "lease_expires_at_ms",
                        SqlValue::TimestampMillis(Self::millis(lease_expires_at)),
                    )
                    .bind("batch", SqlValue::I64(i64::from(command.batch))),
            )
        );
        transaction.commit().await.map_err(map_commit_failure)?;
        Ok(rows.into_iter().map(|row| row.0).collect())
    }

    async fn mark_outbox_dispatched(
        &self,
        id: Uuid,
        now: OffsetDateTime,
    ) -> Result<(), StoreError> {
        let affected = self
            .client
            .execute(
                Statement::new(sql::MARK_OUTBOX_DISPATCHED)
                    .bind("id", SqlValue::Uuid(id))
                    .bind("now_ms", SqlValue::TimestampMillis(Self::millis(now))),
            )
            .await
            .map_err(map_store_error)?;
        if affected == 1 {
            Ok(())
        } else {
            Err(StoreError::NotFound)
        }
    }

    async fn release_outbox(
        &self,
        id: Uuid,
        available_at: OffsetDateTime,
        error: &str,
    ) -> Result<(), StoreError> {
        let affected = self
            .client
            .execute(
                Statement::new(sql::RELEASE_OUTBOX)
                    .bind("id", SqlValue::Uuid(id))
                    .bind(
                        "available_at_ms",
                        SqlValue::TimestampMillis(Self::millis(available_at)),
                    )
                    .bind("last_error", SqlValue::Text(error.to_owned())),
            )
            .await
            .map_err(map_store_error)?;
        if affected == 1 {
            Ok(())
        } else {
            Err(StoreError::NotFound)
        }
    }

    async fn gc_expired(&self, command: &GcExpired) -> Result<GcReport, StoreError> {
        let mut transaction = self
            .client
            .begin(Isolation::ReadCommitted)
            .await
            .map_err(map_store_error)?;
        let idempotency_records = tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::GC_IDEMPOTENCY)
                    .bind(
                        "now_ms",
                        SqlValue::TimestampMillis(Self::millis(command.now)),
                    )
                    .bind("batch", SqlValue::I64(i64::from(command.batch))),
            )
        );
        let outbox_messages = tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::GC_OUTBOX)
                    .bind("batch", SqlValue::I64(i64::from(command.batch))),
            )
        );
        transaction.commit().await.map_err(map_commit_failure)?;
        Ok(GcReport {
            idempotency_records,
            outbox_messages,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use aex_control_app::ports::{
        ControlStore, CreateOrganizationTx, IdempotencyRecordKey, TxOutcome,
    };
    use aex_control_domain::{
        ActorKind, AuditEvent, AuditOutcome, IdempotencyKeyKind, IntentHash, PrincipalKindTag,
        ResourceKind, ScopeKind, Slug,
    };
    use aex_rds_data::{
        DataApiClient, DataApiConfig, DatabaseName, ExecuteResponse, ResourceArn, SecretArn,
        TransactionId, Transport, TransportError,
    };
    use async_trait::async_trait;
    use aws_sdk_rdsdata::types::SqlParameter;
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    use super::AuroraControlStore;
    use crate::sql;

    #[derive(Debug)]
    struct ScriptedTransport {
        statements: Mutex<Vec<String>>,
        commit_is_unknown: bool,
    }

    #[async_trait]
    impl Transport for ScriptedTransport {
        async fn execute(
            &self,
            statement: &str,
            _parameters: Vec<SqlParameter>,
            _transaction: Option<&TransactionId>,
        ) -> Result<ExecuteResponse, TransportError> {
            self.statements
                .lock()
                .expect("statement ledger")
                .push(statement.to_owned());
            Ok(ExecuteResponse {
                records: Vec::new(),
                rows_affected: 1,
            })
        }

        async fn begin(&self) -> Result<TransactionId, TransportError> {
            Ok(TransactionId::new("tx-control"))
        }

        async fn commit(&self, _transaction: &TransactionId) -> Result<String, TransportError> {
            if self.commit_is_unknown {
                Err(TransportError::Indeterminate {
                    message: "response lost".to_owned(),
                })
            } else {
                Ok("Transaction Committed".to_owned())
            }
        }

        async fn rollback(&self, _transaction: &TransactionId) -> Result<(), TransportError> {
            Ok(())
        }
    }

    fn store(commit_is_unknown: bool) -> (AuroraControlStore, Arc<ScriptedTransport>) {
        let transport = Arc::new(ScriptedTransport {
            statements: Mutex::new(Vec::new()),
            commit_is_unknown,
        });
        let config = DataApiConfig::new(
            ResourceArn::parse("arn:aws:rds:eu-west-1:000000000000:cluster:aex").expect("arn"),
            SecretArn::parse("arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex-x")
                .expect("arn"),
            DatabaseName::parse("aex").expect("database"),
        );
        (
            AuroraControlStore::new(DataApiClient::new(transport.clone(), config)),
            transport,
        )
    }

    fn create_organization() -> CreateOrganizationTx {
        let now = OffsetDateTime::UNIX_EPOCH;
        CreateOrganizationTx {
            preassigned_id: Uuid::from_u128(7),
            preassigned_membership_id: Uuid::from_u128(8),
            name: "Acme".to_owned(),
            slug: Slug::parse("acme").expect("slug"),
            created_by_user_id: Uuid::from_u128(9),
            idempotency: IdempotencyRecordKey {
                id: Uuid::from_u128(10),
                key_kind: IdempotencyKeyKind::IdempotencyKey,
                key_value: "create-acme".to_owned(),
                principal_kind: PrincipalKindTag::AccountActor,
                principal_id: Uuid::from_u128(9),
                scope_kind: ScopeKind::Organization,
                scope_id: Uuid::from_u128(9),
                method: aex_wire::types::HttpMethod::Post,
                route: "/api/organizations".to_owned(),
                intent_hash: IntentHash::from_bytes([4; 32]),
                expires_at: now + Duration::days(1),
            },
            audit: AuditEvent {
                id: Uuid::from_u128(11),
                organization_id: Some(Uuid::from_u128(7)),
                workspace_id: None,
                actor_kind: ActorKind::User,
                actor_id: Some(Uuid::from_u128(9)),
                action: "organization.create".to_owned(),
                resource_kind: ResourceKind::Organization,
                resource_id: Some(Uuid::from_u128(7)),
                outcome: AuditOutcome::Allowed,
                request_id: "req-1".to_owned(),
                operation_id: None,
                detail: serde_json::json!({ "slug": "acme" }),
                occurred_at: now,
            },
            now,
        }
    }

    #[tokio::test]
    async fn organization_creation_is_one_transaction_with_finance_audit_and_replay_record() {
        let (store, transport) = store(false);
        let outcome = store
            .create_organization(&create_organization())
            .await
            .expect("created");
        assert!(matches!(outcome, TxOutcome::Committed(_)));
        assert_eq!(
            transport.statements.lock().expect("ledger").as_slice(),
            [
                "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
                sql::FIND_IDEMPOTENCY,
                sql::INSERT_IDEMPOTENCY,
                sql::INSERT_ORGANIZATION,
                sql::INSERT_MEMBERSHIP,
                sql::ENSURE_FINANCE_ACCOUNT,
                sql::INSERT_AUDIT,
                sql::COMPLETE_IDEMPOTENCY,
            ]
        );
    }

    #[tokio::test]
    async fn a_lost_organization_commit_keeps_the_preassigned_reconciliation_identity() {
        let (store, _) = store(true);
        let outcome = store
            .create_organization(&create_organization())
            .await
            .expect("unknown is typed");
        let TxOutcome::Unknown(unknown) = outcome else {
            panic!("lost commit must stay unknown");
        };
        assert_eq!(
            unknown.identity.ceremony,
            aex_control_app::use_cases::ceremony::CREATE_ORGANIZATION
        );
        assert_eq!(unknown.identity.id, Uuid::from_u128(7));
    }
}
