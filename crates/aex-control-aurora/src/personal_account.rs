//! Aurora transaction for the one first-login account aggregate.

use async_trait::async_trait;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use aex_control_app::personal_account::{
    CEREMONY, PersonalAccountProvision, PersonalAccountProvisionCommand, PersonalAccountProvisioner,
};
use aex_control_app::ports::{ReconcileIdentity, StoreError, TxOutcome, UnknownCommit};
use aex_rds_data::{Isolation, SqlValue, Statement};

use crate::error::{map_commit_failure, map_store_error};
use crate::rows::{PersonalAccountProvisionRow, SingleColumnRow};
use crate::{AuroraControlStore, sql};

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

fn millis(instant: OffsetDateTime) -> i64 {
    i64::try_from(instant.unix_timestamp_nanos().div_euclid(1_000_000)).unwrap_or(i64::MAX)
}

fn hex(bytes: &[u8; 32]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut rendered = String::with_capacity(64);
    for byte in bytes {
        rendered.push(char::from(DIGITS[usize::from(byte >> 4)]));
        rendered.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    rendered
}

fn account_slug(id: Uuid) -> String {
    format!("personal-{}", id.simple())
}

impl AuroraControlStore {
    async fn personal_account_in(
        transaction: &mut aex_rds_data::Transaction<'_>,
        user_id: Uuid,
    ) -> Result<Option<PersonalAccountProvision>, aex_rds_data::DataApiError> {
        transaction
            .query_opt::<PersonalAccountProvisionRow>(
                Statement::new(sql::GET_PERSONAL_ACCOUNT).bind("user_id", SqlValue::Uuid(user_id)),
            )
            .await
            .map(|row| row.map(|row| row.0))
    }

    async fn insert_first_login_rows(
        transaction: &mut aex_rds_data::Transaction<'_>,
        command: &PersonalAccountProvisionCommand,
    ) -> Result<(), aex_rds_data::DataApiError> {
        Self::insert_personal_authorities(transaction, command).await?;
        Self::insert_workspace_intent(transaction, command).await?;
        Self::insert_personal_marker_and_events(transaction, command).await
    }

    async fn insert_personal_authorities(
        transaction: &mut aex_rds_data::Transaction<'_>,
        command: &PersonalAccountProvisionCommand,
    ) -> Result<(), aex_rds_data::DataApiError> {
        let ids = command.ids;
        let now_ms = millis(command.now);
        transaction
            .execute(
                Statement::new(sql::INSERT_ORGANIZATION)
                    .bind("id", SqlValue::Uuid(ids.account_id))
                    .bind("name", SqlValue::Text("Personal".to_owned()))
                    .bind("slug", SqlValue::Text(account_slug(ids.account_id)))
                    .bind("created_by_user_id", SqlValue::Uuid(command.user_id))
                    .bind("now_ms", SqlValue::TimestampMillis(now_ms)),
            )
            .await?;
        transaction
            .execute(
                Statement::new(sql::INSERT_MEMBERSHIP)
                    .bind("id", SqlValue::Uuid(ids.membership_id))
                    .bind("organization_id", SqlValue::Uuid(ids.account_id))
                    .bind("user_id", SqlValue::Uuid(command.user_id))
                    .bind("role", SqlValue::Text("owner".to_owned()))
                    .bind("now_ms", SqlValue::TimestampMillis(now_ms)),
            )
            .await?;
        // Finance precedes the workspace so billing-account insertion cannot
        // fan out an account-state projection with a database-generated UUID.
        // The workspace-provision projection below carries the current state.
        transaction
            .query_one::<SingleColumnRow>(
                Statement::new(sql::ENSURE_PERSONAL_LEDGER_ACCOUNTS)
                    .bind("account_id", SqlValue::Uuid(ids.account_id))
                    .bind(
                        "available_account_id",
                        SqlValue::Uuid(ids.available_account_id),
                    )
                    .bind(
                        "reserved_account_id",
                        SqlValue::Uuid(ids.reserved_account_id),
                    ),
            )
            .await?;
        Ok(())
    }

    async fn insert_workspace_intent(
        transaction: &mut aex_rds_data::Transaction<'_>,
        command: &PersonalAccountProvisionCommand,
    ) -> Result<(), aex_rds_data::DataApiError> {
        let ids = command.ids;
        let now_ms = millis(command.now);
        transaction
            .execute(
                Statement::new(sql::INSERT_WORKSPACE)
                    .bind("id", SqlValue::Uuid(ids.workspace_id))
                    .bind("organization_id", SqlValue::Uuid(ids.account_id))
                    .bind("name", SqlValue::Text("Workspace".to_owned()))
                    .bind("slug", SqlValue::Text("workspace".to_owned()))
                    .bind("region", SqlValue::Text("eu-west-1".to_owned()))
                    .bind("operation_id", SqlValue::Uuid(ids.operation_id))
                    .bind("created_by_user_id", SqlValue::Uuid(command.user_id))
                    .bind("now_ms", SqlValue::TimestampMillis(now_ms)),
            )
            .await?;
        transaction
            .execute(
                Statement::new(sql::INSERT_IDEMPOTENCY)
                    .bind("id", SqlValue::Uuid(ids.idempotency_id))
                    .bind("key_kind", SqlValue::Text("idempotency_key".to_owned()))
                    .bind(
                        "key_value",
                        SqlValue::Text(format!("first-login:{}", command.user_id)),
                    )
                    .bind("principal_kind", SqlValue::Text("account_actor".to_owned()))
                    .bind("principal_id", SqlValue::Uuid(command.user_id))
                    .bind("scope_kind", SqlValue::Text("organization".to_owned()))
                    .bind("scope_id", SqlValue::Uuid(ids.account_id))
                    .bind("method", SqlValue::Text("POST".to_owned()))
                    .bind("route", SqlValue::Text("internal:first-login".to_owned()))
                    .bind(
                        "intent_hash",
                        SqlValue::Bytes(command.intent_hash.as_bytes().to_vec()),
                    )
                    .bind("now_ms", SqlValue::TimestampMillis(now_ms))
                    .bind(
                        "expires_at_ms",
                        SqlValue::TimestampMillis(millis(command.now + Duration::days(1))),
                    ),
            )
            .await?;
        Ok(())
    }

    async fn insert_personal_marker_and_events(
        transaction: &mut aex_rds_data::Transaction<'_>,
        command: &PersonalAccountProvisionCommand,
    ) -> Result<(), aex_rds_data::DataApiError> {
        let ids = command.ids;
        let now_ms = millis(command.now);
        transaction
            .execute(
                Statement::new(sql::INSERT_OPERATION)
                    .bind("id", SqlValue::Uuid(ids.operation_id))
                    .bind("kind", SqlValue::Text("workspace_provision".to_owned()))
                    .bind("visibility", SqlValue::Text("internal".to_owned()))
                    .bind("organization_id", SqlValue::Uuid(ids.account_id))
                    .bind("workspace_id", SqlValue::Uuid(ids.workspace_id))
                    .bind("principal_kind", SqlValue::Text("account_actor".to_owned()))
                    .bind("principal_id", SqlValue::Uuid(command.user_id))
                    .bind(
                        "scopes",
                        SqlValue::Json(serde_json::json!(["account:write"])),
                    )
                    .bind(
                        "intent_hash",
                        SqlValue::Bytes(command.intent_hash.as_bytes().to_vec()),
                    )
                    .bind("now_ms", SqlValue::TimestampMillis(now_ms)),
            )
            .await?;
        transaction
            .execute(
                Statement::new(sql::ATTACH_IDEMPOTENCY_OPERATION)
                    .bind("id", SqlValue::Uuid(ids.idempotency_id))
                    .bind("operation_id", SqlValue::Uuid(ids.operation_id)),
            )
            .await?;
        transaction
            .execute(
                Statement::new(sql::INSERT_PERSONAL_ACCOUNT)
                    .bind("account_id", SqlValue::Uuid(ids.account_id))
                    .bind("user_id", SqlValue::Uuid(command.user_id))
                    .bind("membership_id", SqlValue::Uuid(ids.membership_id))
                    .bind("workspace_id", SqlValue::Uuid(ids.workspace_id))
                    .bind("now_ms", SqlValue::TimestampMillis(now_ms)),
            )
            .await?;
        transaction
            .execute(
                Statement::new(sql::INSERT_AUDIT)
                    .bind("id", SqlValue::Uuid(ids.audit_id))
                    .bind("organization_id", SqlValue::Uuid(ids.account_id))
                    .bind("workspace_id", SqlValue::Uuid(ids.workspace_id))
                    .bind("actor_kind", SqlValue::Text("user".to_owned()))
                    .bind("actor_id", SqlValue::Uuid(command.user_id))
                    .bind(
                        "action",
                        SqlValue::Text("personal_account.provisioned".to_owned()),
                    )
                    .bind("resource_kind", SqlValue::Text("workspace".to_owned()))
                    .bind("resource_id", SqlValue::Uuid(ids.workspace_id))
                    .bind("outcome", SqlValue::Text("allowed".to_owned()))
                    .bind("request_id", SqlValue::Text("first-login".to_owned()))
                    .bind("operation_id", SqlValue::Uuid(ids.operation_id))
                    .bind("detail", SqlValue::Json(serde_json::json!({})))
                    .bind("occurred_at_ms", SqlValue::TimestampMillis(now_ms)),
            )
            .await?;

        let payload = serde_json::json!({
            "workspaceId": ids.workspace_id,
            "organizationId": ids.account_id,
            "region": "eu-west-1",
            "operationId": ids.operation_id,
            "idempotencyId": ids.idempotency_id,
            "intentHash": hex(command.intent_hash.as_bytes()),
        });
        transaction
            .execute(
                Statement::new(sql::INSERT_OUTBOX)
                    .bind("id", SqlValue::Uuid(ids.outbox_id))
                    .bind(
                        "topic",
                        SqlValue::Text("workspace.provision.requested".to_owned()),
                    )
                    .bind("dedupe_key", SqlValue::Text(ids.workspace_id.to_string()))
                    .bind("group_key", SqlValue::Text(ids.account_id.to_string()))
                    .bind("payload", SqlValue::Json(payload))
                    .bind("attempts", SqlValue::I64(0))
                    .bind("available_at_ms", SqlValue::TimestampMillis(now_ms))
                    .bind("created_at_ms", SqlValue::TimestampMillis(now_ms)),
            )
            .await?;
        Ok(())
    }
}

#[async_trait]
impl PersonalAccountProvisioner for AuroraControlStore {
    async fn provision_personal_account(
        &self,
        command: &PersonalAccountProvisionCommand,
    ) -> Result<TxOutcome<PersonalAccountProvision>, StoreError> {
        let mut transaction = self
            .client
            .begin(Isolation::ReadCommitted)
            .await
            .map_err(map_store_error)?;
        tx_try!(
            transaction,
            transaction.query_one::<SingleColumnRow>(
                Statement::new(sql::LOCK_PERSONAL_ACCOUNT)
                    .bind("user_id", SqlValue::Uuid(command.user_id)),
            )
        );
        if let Some(existing) = tx_try!(
            transaction,
            Self::personal_account_in(&mut transaction, command.user_id)
        ) {
            let _ = transaction.rollback().await;
            return Ok(TxOutcome::Replayed(existing));
        }

        tx_try!(
            transaction,
            Self::insert_first_login_rows(&mut transaction, command)
        );
        let created = tx_try!(
            transaction,
            Self::personal_account_in(&mut transaction, command.user_id)
        );
        let Some(created) = created else {
            let _ = transaction.rollback().await;
            return Err(StoreError::Decode(
                "personal account insert was not readable".to_owned(),
            ));
        };

        match transaction.commit().await {
            Ok(_) => Ok(TxOutcome::Committed(created)),
            Err(failure) => match map_commit_failure(failure) {
                StoreError::Unknown => Ok(TxOutcome::Unknown(UnknownCommit {
                    identity: ReconcileIdentity {
                        ceremony: CEREMONY,
                        id: command.user_id,
                    },
                })),
                error => Err(error),
            },
        }
    }
}
